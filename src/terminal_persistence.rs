//! Local terminal snapshots and exact native conversation identities.
//! Hooks record conversation IDs and permission presets, never raw hook payloads.
use crate::app::ProviderKind;
use anyhow::{Context, Result, ensure};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use directories::{BaseDirs, ProjectDirs};
use fs2::FileExt;
use portable_pty::CommandBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Mutex,
};

const MAX_FILE: u64 = 4 * 1024 * 1024;
const MAX_HISTORY: usize = 1024 * 1024;
const KEY_ENV: &str = "VIBE_TERMINAL_KEY";
const PROVIDER_ENV: &str = "VIBE_TERMINAL_PROVIDER";
const HELPER_ENV: &str = "VIBE_TERMINAL_HELPER";
const RUN_ENV: &str = "VIBE_TERMINAL_RUN";

/// Native hook modes, not guessed terminal output or model-generated text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum CodexPermissions {
    #[serde(rename = "default")]
    Workspace,
    #[serde(rename = "acceptEdits")]
    AutoReview,
    #[serde(rename = "plan")]
    ReadOnly,
    #[serde(rename = "dontAsk")]
    WorkspaceNoPrompts,
    #[serde(rename = "bypassPermissions")]
    FullAccess,
}

impl CodexPermissions {
    fn apply(self, command: &mut CommandBuilder) {
        let (sandbox, approval, reviewer) = match self {
            Self::Workspace => ("workspace-write", "on-request", "user"),
            Self::AutoReview => ("workspace-write", "on-request", "auto_review"),
            Self::ReadOnly => ("read-only", "on-request", "user"),
            Self::WorkspaceNoPrompts => ("workspace-write", "never", "user"),
            Self::FullAccess => ("danger-full-access", "never", "user"),
        };
        command.args(["--sandbox", sandbox, "--ask-for-approval", approval]);
        command.args(["-c", &format!("approvals_reviewer=\"{reviewer}\"")]);
    }
}

pub(crate) fn clear_hook_environment(command: &mut CommandBuilder) {
    for name in [KEY_ENV, PROVIDER_ENV, HELPER_ENV, RUN_ENV] {
        command.env_remove(name);
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TerminalArchive {
    version: u32,
    pub provider: ProviderKind,
    rows: u16,
    cols: u16,
    history: Vec<HistoryRow>,
    state: String,
    scrollback: usize,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct HistoryRow {
    text: String,
    wrapped: bool,
}

pub struct TerminalStore {
    root: PathBuf,
    pub key: String,
    pub provider: ProviderKind,
    writer: Mutex<()>,
    run: Mutex<Option<String>>,
    pub error: Mutex<Option<String>>,
}

fn root() -> Result<PathBuf> {
    Ok(ProjectDirs::from("dev", "Vibe", "Vibe")
        .context("CLI mentési könyvtár nem érhető el")?
        .data_local_dir()
        .join("terminals"))
}

pub fn valid_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok() && value.len() == 36
}

impl TerminalStore {
    pub fn open(key: &str, provider: ProviderKind) -> Result<Self> {
        Self::at(root()?, key, provider)
    }
    pub(crate) fn at(root: PathBuf, key: &str, provider: ProviderKind) -> Result<Self> {
        ensure!(
            valid_id(key) && provider.is_terminal(),
            "Érvénytelen CLI mentési azonosító"
        );
        Ok(Self {
            root,
            key: key.into(),
            provider,
            writer: Mutex::new(()),
            run: Mutex::new(None),
            error: Mutex::new(None),
        })
    }
    fn path(&self) -> PathBuf {
        self.root.join(format!("{}.json", self.key))
    }
    fn meta_path(&self) -> PathBuf {
        self.root.join(format!("{}.session.json", self.key))
    }
    fn owner_path(&self) -> PathBuf {
        self.root.join(format!("{}.owner", self.key))
    }
    fn disk_lock(&self) -> Result<fs::File> {
        fs::create_dir_all(&self.root)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(self.root.join(format!("{}.lock", self.key)))?;
        lock.lock_exclusive()?;
        Ok(lock)
    }
    fn begin_run(&self) -> Result<String> {
        let _lock = self.disk_lock()?;
        let run = uuid::Uuid::new_v4().to_string();
        atomic_write(&self.owner_path(), run.as_bytes())?;
        *self.run.lock().unwrap_or_else(|error| error.into_inner()) = Some(run.clone());
        Ok(run)
    }
    fn owns_run(&self, run: &str) -> Result<bool> {
        Ok(read_bounded(&self.owner_path(), 128)?.is_some_and(|bytes| bytes == run.as_bytes()))
    }
    pub fn native_home(&self) -> PathBuf {
        self.root.join(format!("{}.codex", self.key))
    }
    pub fn load(&self) -> Result<Option<TerminalArchive>> {
        let Some(bytes) = read_bounded(&self.path(), MAX_FILE)? else {
            return Ok(None);
        };
        let archive: TerminalArchive =
            serde_json::from_slice(&bytes).context("CLI mentés érvénytelen")?;
        archive.validate()?;
        ensure!(
            archive.provider == self.provider,
            "CLI mentés szolgáltatója eltér"
        );
        Ok(Some(archive))
    }
    pub fn session_id(&self) -> Result<Option<String>> {
        let Some(bytes) = read_bounded(&self.meta_path(), 4096)? else {
            return Ok(None);
        };
        let value: Value =
            serde_json::from_slice(&bytes).context("CLI munkamenetadat érvénytelen")?;
        ensure!(
            value["provider"] == serde_json::to_value(self.provider)?,
            "CLI munkamenet szolgáltatója eltér"
        );
        let id = value["session_id"]
            .as_str()
            .context("CLI munkamenetazonosító hiányzik")?;
        ensure!(valid_id(id), "CLI munkamenetazonosító érvénytelen");
        Ok(Some(id.into()))
    }
    pub fn save(&self, screen: &vt100::Screen) -> Result<()> {
        let _guard = self
            .writer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let result = (|| {
            let _lock = self.disk_lock()?;
            if let Some(run) = self
                .run
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_deref()
                && !self.owns_run(run)?
            {
                return Ok(());
            }
            self.load()?; // Preserve invalid or future-version snapshots.
            let archive = TerminalArchive::capture(screen, self.provider);
            archive.validate()?;
            let bytes = serde_json::to_vec(&archive)?;
            ensure!(
                bytes.len() as u64 <= MAX_FILE,
                "CLI mentés meghaladja a 4 MiB korlátot"
            );
            atomic_write(&self.path(), &bytes)
        })();
        *self.error.lock().unwrap_or_else(|error| error.into_inner()) =
            result.as_ref().err().map(ToString::to_string);
        result
    }
    pub fn configure(&self, command: &mut CommandBuilder, codex: bool) -> Result<()> {
        let helper = helper_executable()?;
        let run = self.begin_run()?;
        command.env(RUN_ENV, &run);
        command.env(KEY_ENV, &self.key);
        command.env(PROVIDER_ENV, if codex { "codex" } else { "antigravity" });
        command.env(HELPER_ENV, &helper);
        if codex {
            // Every panel shares the last explicitly changed Codex preset.
            // Without a saved choice the CLI's own config/defaults take effect.
            if let Some(bytes) = read_bounded(&self.root.join("codex-permissions.json"), 4096)? {
                let mode: CodexPermissions = serde_json::from_slice(&bytes)
                    .context("A mentett Codex jogosultsági mód érvénytelen")?;
                mode.apply(command);
                atomic_write(
                    &self.permission_observation_path(),
                    &serde_json::to_vec(&json!({"run":run,"mode":mode}))?,
                )?;
            }
            let mut hooks = serde_json::Map::new();
            hooks.insert("SessionStart".into(), json!([{"matcher":"startup|resume|clear|fork","hooks":[{"type":"command","command":hook_command(),"timeout":3}]}]));
            // A hook trusted after startup still records this conversation on
            // the next completed turn or normal exit. Never bypass hook trust.
            for event in ["UserPromptSubmit", "Stop", "Interrupt", "SessionEnd"] {
                hooks.insert(
                    event.into(),
                    json!([{"hooks":[{"type":"command","command":hook_command(),"timeout":3}]}]),
                );
            }
            let home = command
                .get_env("CODEX_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| self.native_home());
            install_codex_hooks(&home, hooks)?;
            if let Some(id) = self.session_id()? {
                command.args(["resume", &id]);
            } else if self.load()?.is_some() {
                command.arg("resume");
            }
        } else {
            install_google_hook()?;
            if let Some(id) = self.session_id()? {
                command.args(["--conversation", &id]);
            }
        }
        Ok(())
    }

    fn permission_observation_path(&self) -> PathBuf {
        self.root.join(format!("{}.permissions.json", self.key))
    }

    fn record_permissions(&self, payload: &Value) -> Result<()> {
        if self.provider != ProviderKind::Chatgpt {
            return Ok(());
        }
        let Some(mode) = payload
            .get("permission_mode")
            .and_then(|mode| serde_json::from_value::<CodexPermissions>(mode.clone()).ok())
        else {
            return Ok(());
        };
        let Some(event) = payload["hook_event_name"].as_str() else {
            return Ok(());
        };
        if !matches!(
            event,
            "SessionStart" | "UserPromptSubmit" | "Stop" | "Interrupt"
        ) {
            return Ok(());
        }
        let Some(run) = read_bounded(&self.owner_path(), 128)? else {
            return Ok(());
        };
        let run = String::from_utf8(run)?;
        let observation = read_bounded(&self.permission_observation_path(), 4096)?
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
        let previous = observation
            .as_ref()
            .filter(|value| value["run"] == run)
            .and_then(|value| {
                serde_json::from_value::<CodexPermissions>(value["mode"].clone()).ok()
            });
        if previous == Some(mode) {
            return Ok(());
        }
        // SessionStart establishes a baseline, not a new user choice. Thus an
        // older idle panel cannot undo a mode selected in another panel.
        if event != "SessionStart" {
            let lock = fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(self.root.join("codex-permissions.lock"))?;
            lock.lock_exclusive()?;
            let path = self.root.join("codex-permissions.json");
            if previous.is_some() || !path.exists() {
                atomic_write(&path, &serde_json::to_vec(&mode)?)?;
            }
        }
        atomic_write(
            &self.permission_observation_path(),
            &serde_json::to_vec(&json!({"run":run,"mode":mode}))?,
        )
    }
}

impl TerminalArchive {
    pub fn capture(screen: &vt100::Screen, provider: ProviderKind) -> Self {
        let mut copy = screen.clone();
        let mut history = Vec::new();
        let (_, cols) = screen.size();
        let mut bytes = 0;
        // vt100::scrollback_len is capacity, not the populated row count.
        copy.set_scrollback(usize::MAX);
        let populated = copy.scrollback();
        for offset in 1..=populated.min(1000) {
            copy.set_scrollback(offset);
            let row = copy.rows(0, cols).next().unwrap_or_default();
            if bytes + row.len() > MAX_HISTORY {
                break;
            }
            bytes += row.len();
            history.push(HistoryRow {
                text: row,
                wrapped: copy.row_wrapped(0),
            });
        }
        history.reverse();
        copy.set_scrollback(0);
        let (rows, cols) = copy.size();
        Self {
            version: 1,
            provider,
            rows,
            cols,
            history,
            state: STANDARD.encode(copy.state_formatted()),
            scrollback: screen.scrollback(),
        }
    }
    fn validate(&self) -> Result<()> {
        ensure!(
            self.version == 1 && self.provider.is_terminal(),
            "Nem támogatott CLI mentés"
        );
        ensure!(
            (1..=1000).contains(&self.rows) && (1..=1000).contains(&self.cols),
            "CLI mentés mérete érvénytelen"
        );
        ensure!(
            self.history.len() <= 1000
                && self.history.iter().map(|row| row.text.len()).sum::<usize>() <= MAX_HISTORY,
            "CLI előzmény túl nagy"
        );
        ensure!(
            self.history
                .iter()
                .all(|line| !line.text.chars().any(char::is_control)),
            "CLI előzmény vezérlőkaraktert tartalmaz"
        );
        ensure!(self.state.len() <= 3 * 1024 * 1024, "CLI képernyő túl nagy");
        STANDARD
            .decode(&self.state)
            .context("CLI képernyőkódolás érvénytelen")?;
        Ok(())
    }
    pub fn replay(&self) -> Result<vt100::Screen> {
        self.validate()?;
        let mut parser = vt100::Parser::new(self.rows, self.cols, 1000);
        for row in &self.history {
            parser.process(row.text.as_bytes());
            if row.wrapped {
                let width = unicode_width::UnicodeWidthStr::width(row.text.as_str());
                for _ in width..usize::from(self.cols) {
                    parser.process(b" ");
                }
                parser.process(b" \x08"); // trigger the original soft wrap, then return to column zero
            } else {
                parser.process(b"\r\n");
            }
        }
        // Push all restored history above the viewport before painting its state.
        for _ in 1..self.rows {
            parser.process(b"\r\n");
        }
        parser.process(&STANDARD.decode(&self.state)?);
        parser
            .screen_mut()
            .set_scrollback(self.scrollback.min(self.history.len()));
        Ok(parser.screen().clone())
    }
}

fn read_bounded(path: &Path, limit: u64) -> Result<Option<Vec<u8>>> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("CLI mentés nem olvasható"),
    };
    let mut bytes = Vec::new();
    file.take(limit + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= limit, "CLI mentési fájl túl nagy");
    Ok(Some(bytes))
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("CLI mentési útvonal érvénytelen")?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    temporary.write_all(bytes)?;
    temporary.as_file().sync_all()?;
    temporary.persist(path).map_err(|error| error.error)?;
    Ok(())
}

pub fn hook_command() -> String {
    #[cfg(windows)]
    {
        // Codex can wrap hooks in PowerShell. Nested cmd /c loses the quotes
        // around an expanded executable path with spaces. EncodedCommand is
        // one shell-neutral argument; the path stays an environment value.
        let script = "if ($env:VIBE_TERMINAL_KEY -and $env:VIBE_TERMINAL_HELPER) { & $env:VIBE_TERMINAL_HELPER --record-cli-session; exit $LASTEXITCODE }";
        let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
        format!(
            "powershell.exe -NoLogo -NoProfile -NonInteractive -EncodedCommand {}",
            STANDARD.encode(bytes)
        )
    }
    #[cfg(not(windows))]
    {
        "if [ -n \"$VIBE_TERMINAL_KEY\" ]; then \"$VIBE_TERMINAL_HELPER\" --record-cli-session; fi"
            .into()
    }
}

fn helper_executable() -> Result<PathBuf> {
    // The running executable also implements the hidden hook mode. The fixed
    // hook command resolves this path from each VIBE-launched CLI's environment,
    // so updates/renames do not leave a global hook pointing to an old binary.
    std::env::current_exe().context("VIBE munkamenet-segédprogram nem található")
}

fn install_codex_hooks(home: &Path, hooks: serde_json::Map<String, Value>) -> Result<()> {
    // A real source path stays stable across working directories and restarts.
    // Codex owns persistence of trust and disabled choices in config.toml.
    fs::create_dir_all(home)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(home.join("vibe-hooks.lock"))?;
    lock.lock_exclusive()?;
    let path = home.join("hooks.json");
    let original = read_bounded(&path, MAX_FILE)?;
    let mut value: Value = match &original {
        Some(bytes) => {
            serde_json::from_slice(bytes).context("Invalid Codex hooks.json; original preserved")?
        }
        None => json!({}),
    };
    let events = value
        .as_object_mut()
        .context("Codex hooks.json must be an object")?
        .entry("hooks")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .context("Codex hooks must be an object")?;
    for (event, groups) in hooks {
        let existing = events
            .entry(event)
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .context("Codex hook event must be an array")?;
        for group in groups.as_array().expect("generated hook groups") {
            if !existing.contains(group) {
                existing.push(group.clone());
            }
        }
    }
    let bytes = serde_json::to_vec_pretty(&value)?;
    if original.as_deref() != Some(bytes.as_slice()) {
        atomic_write(&path, &bytes)?;
    }
    Ok(())
}

fn install_google_hook() -> Result<()> {
    let base = BaseDirs::new().context("Google CLI konfigurációs könyvtár nem érhető el")?;
    let directory = base.home_dir().join(".gemini").join("config");
    fs::create_dir_all(&directory)?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join("vibe-hooks.lock"))?;
    lock.lock_exclusive()?;
    let path = directory.join("hooks.json");
    let mut value = match read_bounded(&path, 1024 * 1024)? {
        Some(bytes) => serde_json::from_slice::<Value>(&bytes)
            .context("Google hooks.json érvénytelen; az eredeti fájl megmaradt")?,
        None => json!({}),
    };
    let object = value
        .as_object_mut()
        .context("Google hooks.json nem JSON objektum")?;
    let hook = json!({
        "SessionStart":[{"type":"command","command":hook_command(),"timeout":3}],
        "Stop":[{"type":"command","command":hook_command(),"timeout":3}]
    });
    if object.get("vibe-cli-session-persistence") != Some(&hook) {
        object.insert("vibe-cli-session-persistence".into(), hook);
        atomic_write(&path, &serde_json::to_vec_pretty(&value)?)?;
    }
    Ok(())
}

/// Hidden executable mode. Outside VIBE-launched CLI processes this is a no-op.
/// There is no GUI initialization, network activity, or model-visible context.
pub fn record_hook() -> Result<()> {
    let Some(key) = std::env::var_os(KEY_ENV) else {
        return Ok(());
    };
    let key = key.to_str().context("CLI mentési kulcs érvénytelen")?;
    let provider = match std::env::var(PROVIDER_ENV).as_deref() {
        Ok("codex") => ProviderKind::Chatgpt,
        Ok("antigravity") => ProviderKind::Antigravity,
        _ => return Ok(()),
    };
    let store = TerminalStore::open(key, provider)?;
    let mut bytes = Vec::new();
    std::io::stdin()
        .take(64 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 64 * 1024, "CLI hook-válasz túl nagy");
    let run = std::env::var(RUN_ENV).context("CLI futási azonosító hiányzik")?;
    let _lock = store.disk_lock()?;
    if !store.owns_run(&run)? {
        return Ok(());
    }
    record_payload(&store, &bytes)?;
    let payload: Value = serde_json::from_slice(&bytes)?;
    if is_turn_completion(provider, &payload) {
        crate::completion_sound::play();
    }
    Ok(())
}

fn is_turn_completion(provider: ProviderKind, payload: &Value) -> bool {
    match provider {
        ProviderKind::Chatgpt => payload["hook_event_name"] == "Stop",
        // Google's Stop payload has no event-name field. These fields belong
        // to Stop; fullyIdle excludes ongoing asynchronous/background work.
        ProviderKind::Antigravity => {
            payload["terminationReason"] == "model_stop"
                && payload["fullyIdle"] == true
                && payload["error"].as_str().unwrap_or_default().is_empty()
        }
        ProviderKind::OpenRouter => false,
    }
}

fn record_payload(store: &TerminalStore, bytes: &[u8]) -> Result<()> {
    let value: Value = serde_json::from_slice(bytes).context("CLI hook-válasz érvénytelen")?;
    let field = if store.provider == ProviderKind::Chatgpt {
        "session_id"
    } else {
        "conversationId"
    };
    let id = value[field]
        .as_str()
        .context("CLI munkamenetazonosító hiányzik")?;
    ensure!(valid_id(id), "CLI munkamenetazonosító érvénytelen");
    store.record_permissions(&value)?;
    atomic_write(
        &store.meta_path(),
        &serde_json::to_vec(&json!({"provider":store.provider,"session_id":id}))?,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_sound_requires_a_finished_turn_not_startup_tools_or_interrupts() {
        assert!(is_turn_completion(
            ProviderKind::Chatgpt,
            &json!({"hook_event_name":"Stop"})
        ));
        for event in [
            "SessionStart",
            "UserPromptSubmit",
            "PostToolUse",
            "Interrupt",
            "SessionEnd",
            "SubagentStop",
        ] {
            assert!(!is_turn_completion(
                ProviderKind::Chatgpt,
                &json!({"hook_event_name":event})
            ));
        }
        assert!(is_turn_completion(
            ProviderKind::Antigravity,
            &json!({"terminationReason":"model_stop","fullyIdle":true,"error":""})
        ));
        for payload in [
            json!({"conversationId":"session-start"}),
            json!({"terminationReason":"model_stop","fullyIdle":false}),
            json!({"terminationReason":"model_stop"}),
            json!({"terminationReason":"error","fullyIdle":true}),
            json!({"terminationReason":"max_steps_exceeded","fullyIdle":true}),
            json!({"terminationReason":"model_stop","fullyIdle":true,"error":"failed"}),
        ] {
            assert!(!is_turn_completion(ProviderKind::Antigravity, &payload));
        }
    }

    #[test]
    fn stable_hook_source_preserves_native_choices_and_other_hooks() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        let original =
            json!({"hooks":{"Stop":[{"hooks":[{"type":"command","command":"other-helper"}]}]}});
        fs::write(
            home.join("hooks.json"),
            serde_json::to_vec(&original).unwrap(),
        )
        .unwrap();
        fs::write(
            home.join("config.toml"),
            "# saved native trust and disabled choices\n",
        )
        .unwrap();
        let hooks = serde_json::from_value(
            json!({"Stop":[{"hooks":[{"type":"command","command":hook_command(),"timeout":3}]}]}),
        )
        .unwrap();
        install_codex_hooks(home, hooks).unwrap();
        let first = fs::read(home.join("hooks.json")).unwrap();
        let hooks = serde_json::from_slice::<Value>(&first).unwrap()["hooks"]
            .as_object()
            .unwrap()
            .clone();
        install_codex_hooks(home, hooks).unwrap();
        assert_eq!(fs::read(home.join("hooks.json")).unwrap(), first);
        assert_eq!(
            serde_json::from_slice::<Value>(&first).unwrap()["hooks"]["Stop"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            fs::read_to_string(home.join("config.toml")).unwrap(),
            "# saved native trust and disabled choices\n"
        );
    }

    #[test]
    fn permissions_survive_new_panels_and_old_idle_panels_cannot_reset_them() {
        let directory = tempfile::tempdir().unwrap();
        let open = || {
            TerminalStore::at(
                directory.path().into(),
                &uuid::Uuid::new_v4().to_string(),
                ProviderKind::Chatgpt,
            )
            .unwrap()
        };
        let first = open();
        let idle = open();
        first.begin_run().unwrap();
        idle.begin_run().unwrap();
        for store in [&first, &idle] {
            store
                .record_permissions(
                    &json!({"hook_event_name":"SessionStart","permission_mode":"plan"}),
                )
                .unwrap();
        }
        assert!(!directory.path().join("codex-permissions.json").exists());
        first.record_permissions(&json!({"hook_event_name":"UserPromptSubmit","permission_mode":"bypassPermissions","secret":"must-not-persist"})).unwrap();
        idle.record_permissions(&json!({"hook_event_name":"Stop","permission_mode":"plan"}))
            .unwrap();
        let saved = fs::read_to_string(directory.path().join("codex-permissions.json")).unwrap();
        assert_eq!(saved, "\"bypassPermissions\"");
        let next = open();
        let mut command = CommandBuilder::new("codex");
        command.env("CODEX_HOME", directory.path().join("codex-home"));
        next.configure(&mut command, true).unwrap();
        let args: Vec<_> = command
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--sandbox", "danger-full-access"])
        );
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--ask-for-approval", "never"])
        );
        assert!(!args.iter().any(|arg| arg.contains("network_access=false")));
        assert!(args.iter().all(|arg| !arg.starts_with("hooks.")));
        // Choosing a more restrictive mode must replace the saved Full Access.
        first
            .record_permissions(&json!({"hook_event_name":"Interrupt","permission_mode":"plan"}))
            .unwrap();
        let mut command = CommandBuilder::new("codex");
        command.env("CODEX_HOME", directory.path().join("codex-home"));
        open().configure(&mut command, true).unwrap();
        assert!(
            command
                .get_argv()
                .windows(2)
                .any(|pair| pair[0] == "--sandbox" && pair[1] == "read-only")
        );
    }

    #[test]
    fn unknown_modes_and_google_hooks_never_set_codex_permissions() {
        let directory = tempfile::tempdir().unwrap();
        for provider in [ProviderKind::Chatgpt, ProviderKind::Antigravity] {
            let store = TerminalStore::at(
                directory.path().into(),
                &uuid::Uuid::new_v4().to_string(),
                provider,
            )
            .unwrap();
            store.begin_run().unwrap();
            store
                .record_permissions(
                    &json!({"hook_event_name":"UserPromptSubmit","permission_mode":"unknown"}),
                )
                .unwrap();
            if provider == ProviderKind::Antigravity {
                store.record_permissions(&json!({"hook_event_name":"UserPromptSubmit","permission_mode":"bypassPermissions"})).unwrap();
            }
        }
        assert!(!directory.path().join("codex-permissions.json").exists());
    }

    #[test]
    fn no_saved_permission_choice_leaves_cli_configuration_in_control() {
        let directory = tempfile::tempdir().unwrap();
        let store = TerminalStore::at(
            directory.path().into(),
            &uuid::Uuid::new_v4().to_string(),
            ProviderKind::Chatgpt,
        )
        .unwrap();
        let mut command = CommandBuilder::new("codex");
        command.env("CODEX_HOME", directory.path().join("codex-home"));
        store.configure(&mut command, true).unwrap();
        assert!(
            !command
                .get_argv()
                .iter()
                .any(|arg| arg == "--sandbox" || arg == "--ask-for-approval")
        );
    }

    #[test]
    #[ignore = "Requires installed Codex CLI; synthetic token and isolated profile, no inference"]
    fn installed_cli_new_panel_loads_remembered_full_access() {
        use crate::terminal_backend::TerminalSession;
        use std::{
            sync::Arc,
            thread,
            time::{Duration, Instant},
        };
        let directory = tempfile::tempdir().unwrap();
        let store = Arc::new(
            TerminalStore::at(
                directory.path().join("terminals"),
                &uuid::Uuid::new_v4().to_string(),
                ProviderKind::Chatgpt,
            )
            .unwrap(),
        );
        store.begin_run().unwrap();
        store.record_permissions(&json!({"hook_event_name":"UserPromptSubmit", "permission_mode":"bypassPermissions"})).unwrap();
        fs::create_dir_all(store.native_home()).unwrap();
        fs::write(
            store.native_home().join("config.toml"),
            "sandbox_mode = \"read-only\"\napproval_policy = \"on-request\"\n",
        )
        .unwrap();
        let mut session = TerminalSession::spawn_persisted(
            directory.path().into(),
            "gpt-6.1-sol".into(),
            crate::app::Effort::default(),
            "vibe-synthetic-not-a-real-token".into(),
            eframe::egui::Context::default(),
            Some(store),
        )
        .unwrap();
        let wait = |session: &mut TerminalSession, text: &str| {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let screen = session.snapshot().contents();
                if screen.contains(text) {
                    break;
                }
                if screen.contains("Hooks need review") {
                    // This fixture's executable is the test runner, not VIBE's
                    // hook helper. Explicitly continue without trusting it.
                    session.write(b"3").unwrap();
                    thread::sleep(Duration::from_millis(200));
                    session.write(b"\r").unwrap();
                    thread::sleep(Duration::from_millis(200));
                    continue;
                }
                assert!(
                    session.exited().is_none() && Instant::now() < deadline,
                    "{screen}"
                );
                thread::sleep(Duration::from_millis(50));
            }
        };
        wait(&mut session, "OpenAI Codex");
        session.write(b"/permissions").unwrap();
        thread::sleep(Duration::from_millis(200));
        session.write(b"\r").unwrap();
        wait(&mut session, "Full Access (current)");
        session.stop();
    }
    #[cfg(windows)]
    #[test]
    fn windows_hook_preserves_spaced_paths_and_stdin_through_both_shells() {
        use std::io::Write as _;
        use std::os::windows::process::CommandExt as _;
        use std::process::{Command, Stdio};

        let directory = tempfile::Builder::new()
            .prefix("vibe hook with spaces ")
            .tempdir()
            .unwrap();
        let helper = directory.path().join("hook helper.cmd");
        fs::write(
            &helper,
            "@echo off\r\nset /p payload=\r\necho %1\r\necho %payload%\r\nexit /b 0\r\n",
        )
        .unwrap();
        for shell in ["cmd.exe", "powershell.exe"] {
            let mut command = Command::new(shell);
            if shell == "cmd.exe" {
                command.args(["/d", "/c", &hook_command()]);
            } else {
                command.args(["-NoProfile", "-NonInteractive", "-Command", &hook_command()]);
            }
            command.creation_flags(0x08000000);
            command.env(KEY_ENV, "fixture").env(HELPER_ENV, &helper);
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = command.spawn().unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(b"{\"session_id\":\"synthetic-input\"}\n")
                .unwrap();
            let output = child.wait_with_output().unwrap();
            assert!(
                output.status.success(),
                "{shell}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let text = String::from_utf8_lossy(&output.stdout);
            assert!(text.contains("--record-cli-session"), "{shell}: {text}");
            assert!(text.contains("synthetic-input"), "{shell}: {text}");

            // A global Google hook outside VIBE must not invoke the helper.
            let output = command.env_remove(KEY_ENV).output().unwrap();
            assert!(output.status.success());
            assert!(output.stdout.is_empty());
        }
    }

    #[test]
    fn terminal_screen_colors_cursor_and_scrollback_survive_restart() {
        let directory = tempfile::tempdir().unwrap();
        let key = uuid::Uuid::new_v4().to_string();
        let store =
            TerminalStore::at(directory.path().into(), &key, ProviderKind::Chatgpt).unwrap();
        let mut parser = vt100::Parser::new(4, 40, 1000);
        for n in 0..14 {
            parser.process(format!("line {n}\r\n").as_bytes());
        }
        parser.process("\x1b[32;1mVálasz 界\x1b[0m\r\n› draft".as_bytes());
        let original = parser.screen().clone();
        store.save(&original).unwrap();
        let mut restored = store.load().unwrap().unwrap().replay().unwrap();
        assert_eq!(restored.contents(), original.contents());
        assert_eq!(restored.cursor_position(), original.cursor_position());
        let mut oldest = original.clone();
        oldest.set_scrollback(usize::MAX);
        let populated = oldest.scrollback();
        restored.set_scrollback(usize::MAX);
        assert_eq!(restored.scrollback(), populated);
        for offset in 0..=populated {
            let mut expected = original.clone();
            expected.set_scrollback(offset);
            restored.set_scrollback(offset);
            assert_eq!(restored.contents(), expected.contents());
        }
    }
    #[test]
    fn native_conversations_stay_separate_and_hook_payloads_are_not_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let first = TerminalStore::at(
            directory.path().into(),
            &uuid::Uuid::new_v4().to_string(),
            ProviderKind::Chatgpt,
        )
        .unwrap();
        let second = TerminalStore::at(
            directory.path().into(),
            &uuid::Uuid::new_v4().to_string(),
            ProviderKind::Antigravity,
        )
        .unwrap();
        let a = uuid::Uuid::new_v4().to_string();
        let b = uuid::Uuid::new_v4().to_string();
        record_payload(
            &first,
            &serde_json::to_vec(&json!({"session_id":a,"secret":"must-not-persist"})).unwrap(),
        )
        .unwrap();
        record_payload(
            &second,
            &serde_json::to_vec(&json!({"conversationId":b,"secret":"must-not-persist"})).unwrap(),
        )
        .unwrap();
        assert_eq!(first.session_id().unwrap(), Some(a));
        assert_eq!(second.session_id().unwrap(), Some(b));
        assert!(
            !fs::read_to_string(first.meta_path())
                .unwrap()
                .contains("must-not-persist")
        );
        assert!(record_payload(&first, br#"{"session_id":"../../other"}"#).is_err());
        assert!(
            TerminalStore::at(directory.path().into(), "../escape", ProviderKind::Chatgpt).is_err()
        );
    }
    #[test]
    fn wrapped_history_and_late_writers_do_not_corrupt_a_restarted_panel() {
        let directory = tempfile::tempdir().unwrap();
        let key = uuid::Uuid::new_v4().to_string();
        let old = TerminalStore::at(directory.path().into(), &key, ProviderKind::Chatgpt).unwrap();
        let old_run = old.begin_run().unwrap();
        let mut original = vt100::Parser::new(3, 12, 1000);
        original.process("hosszú szöveg ismételve több soron át\r\nnext\r\nready".as_bytes());
        old.save(original.screen()).unwrap();
        let archive = old.load().unwrap().unwrap();
        let mut restored = archive.replay().unwrap();
        let mut expected = original.screen().clone();
        expected.set_scrollback(usize::MAX);
        restored.set_scrollback(usize::MAX);
        assert_eq!(restored.contents(), expected.contents());
        let current =
            TerminalStore::at(directory.path().into(), &key, ProviderKind::Chatgpt).unwrap();
        current.begin_run().unwrap();
        let mut fresh = vt100::Parser::new(3, 12, 1000);
        fresh.process(b"new session");
        current.save(fresh.screen()).unwrap();
        old.save(original.screen()).unwrap();
        assert!(!old.owns_run(&old_run).unwrap());
        assert_eq!(
            current
                .load()
                .unwrap()
                .unwrap()
                .replay()
                .unwrap()
                .contents(),
            "new session"
        );
    }

    #[test]
    fn invalid_snapshots_are_preserved_and_ids_are_not_guessed() {
        let directory = tempfile::tempdir().unwrap();
        let store = TerminalStore::at(
            directory.path().into(),
            &uuid::Uuid::new_v4().to_string(),
            ProviderKind::Chatgpt,
        )
        .unwrap();
        assert!(store.load().unwrap().is_none());
        assert!(store.session_id().unwrap().is_none());
        fs::write(store.path(), b"invalid-original").unwrap();
        assert!(store.save(vt100::Parser::new(4, 20, 0).screen()).is_err());
        assert_eq!(fs::read(store.path()).unwrap(), b"invalid-original");
    }
}
