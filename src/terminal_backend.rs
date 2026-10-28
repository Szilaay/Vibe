//! Original interactive Codex and Antigravity CLIs, connected to a native PTY.
//!
//! VIBE only emulates a terminal here. Prompts, slash commands and approvals
//! belong to each CLI; no terminal input is interpreted as a VIBE chat request.
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicIsize, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    thread,
    time::Duration,
};

use crate::terminal_persistence::{TerminalArchive, TerminalStore};
use anyhow::{Context, Result, ensure};
use eframe::egui;
use portable_pty::{Child, ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

const ROWS: u16 = 30;
const COLS: u16 = 100;
// Keep scrollback bounded: publishing a screen must never copy unbounded history.
const SCROLLBACK: usize = 1_000;
const INPUT_QUEUE_CAPACITY: usize = 64;
const QUERY_QUEUE_CAPACITY: usize = 64;
const MAX_WRITE_BYTES: usize = 64 * 1024;
const WORKER_INTERVAL: Duration = Duration::from_millis(8);
static NEXT_INSTANCE: AtomicU64 = AtomicU64::new(1);
type Parser = Arc<Mutex<vt100::Parser<TerminalCallbacks>>>;
type PublishedScreen = Arc<Mutex<Arc<vt100::Screen>>>;
type Exit = Arc<Mutex<Option<String>>>;

struct TerminalControl {
    size: AtomicU32,
    scroll: AtomicIsize,
    revision: AtomicU64,
    stopped: AtomicBool,
}

impl TerminalControl {
    fn new() -> Self {
        Self {
            size: AtomicU32::new(pack_size(ROWS, COLS)),
            scroll: AtomicIsize::new(0),
            revision: AtomicU64::new(0),
            stopped: AtomicBool::new(false),
        }
    }
}

fn pack_size(rows: u16, cols: u16) -> u32 {
    (u32::from(rows) << 16) | u32::from(cols)
}

fn unpack_size(size: u32) -> (u16, u16) {
    ((size >> 16) as u16, size as u16)
}

#[derive(Default)]
struct TerminalCallbacks {
    responses: Vec<u8>,
}

impl vt100::Callbacks for TerminalCallbacks {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        params: &[&[u16]],
        command: char,
    ) {
        let parameter = params
            .first()
            .and_then(|value| value.first())
            .copied()
            .unwrap_or(0);
        match (first, second, command, parameter) {
            (None, None, 'n', 5) => self.responses.extend_from_slice(b"\x1b[0n"),
            (None | Some(b'?'), None, 'n', 6) => {
                let (row, column) = screen.cursor_position();
                let private = if first.is_some() { "?" } else { "" };
                self.responses.extend_from_slice(
                    format!("\x1b[{private}{};{}R", row + 1, column + 1).as_bytes(),
                );
            }
            (None, None, 'c', 0) => self.responses.extend_from_slice(b"\x1b[?1;2c"),
            (Some(b'>'), None, 'c', 0) => self.responses.extend_from_slice(b"\x1b[>0;1;0c"),
            // Explicitly report unsupported modes. In particular, this terminal
            // does not advertise Kitty keyboard or synchronized-output support.
            (Some(b'?'), Some(b'$'), 'p', mode) => self
                .responses
                .extend_from_slice(format!("\x1b[?{mode};0$y").as_bytes()),
            (None, None, 't', 18) => {
                let (rows, cols) = screen.size();
                self.responses
                    .extend_from_slice(format!("\x1b[8;{rows};{cols}t").as_bytes());
            }
            _ => {}
        }
    }

    fn unhandled_osc(&mut self, _: &mut vt100::Screen, params: &[&[u8]]) {
        match params {
            [b"10", b"?"] => self
                .responses
                .extend_from_slice(b"\x1b]10;rgb:d8d8/dede/e9e9\x1b\\"),
            [b"11", b"?"] => self
                .responses
                .extend_from_slice(b"\x1b]11;rgb:0c0c/1010/1818\x1b\\"),
            _ => {}
        }
    }
}

/// One independently owned CLI process and its terminal screen.
pub struct TerminalSession {
    instance: u64,
    input: Option<SyncSender<Vec<u8>>>,
    pub model: String,
    pub effort: crate::app::Effort,
    control: Arc<TerminalControl>,
    published: PublishedScreen,
    cached_screen: Arc<vt100::Screen>,
    exit: Exit,
    killer: Option<Box<dyn ChildKiller + Send + Sync>>,
    #[cfg(unix)]
    process_group: Option<libc::pid_t>,
    repaint: egui::Context,
    saved_screen: Option<Box<Mutex<vt100::Screen>>>,
    archive: Option<Arc<TerminalStore>>,
}

/// Pins dictation to this exact process, even after focus/workspace changes.
pub struct DictationTarget {
    input: SyncSender<Vec<u8>>,
    control: Arc<TerminalControl>,
    exit: Exit,
}

impl DictationTarget {
    pub fn insert(&self, text: &str) -> Result<()> {
        ensure!(
            !self.control.stopped.load(Ordering::Acquire),
            "A diktálás célablaka bezárult."
        );
        ensure!(
            self.exit
                .try_lock()
                .map_err(|_| anyhow::anyhow!("CLI állapot nem olvasható"))?
                .is_none(),
            "A diktálás célfolyamata leállt."
        );
        let text = crate::voice::plain_text(text);
        ensure!(!text.is_empty(), "Nem hallottam beszédet.");
        ensure!(
            text.len() <= MAX_WRITE_BYTES,
            "A diktált szöveg túl hosszú."
        );
        self.input
            .try_send(text.into_bytes())
            .context("A CLI nem fogadta a diktálást")
    }
}

impl TerminalSession {
    pub fn instance_id(&self) -> u64 {
        self.instance
    }
    pub fn dictation_target(&self) -> Result<DictationTarget> {
        ensure!(
            !self.is_restored() && self.exited().is_none(),
            "Indítsd el a CLI-t a diktáláshoz."
        );
        Ok(DictationTarget {
            input: self.input.as_ref().context("A CLI bezárult")?.clone(),
            control: self.control.clone(),
            exit: self.exit.clone(),
        })
    }
    /// Use the same VIBE-owned native account that supplies the usage meter.
    pub fn spawn_native(
        cwd: PathBuf,
        model: String,
        effort: crate::app::Effort,
        repaint: egui::Context,
    ) -> Result<Self> {
        Self::spawn_native_persisted(cwd, model, effort, repaint, None)
    }
    pub fn spawn_native_persisted(
        cwd: PathBuf,
        model: String,
        effort: crate::app::Effort,
        repaint: egui::Context,
        archive: Option<Arc<TerminalStore>>,
    ) -> Result<Self> {
        ensure!(cwd.is_dir(), "A Codex projektkönyvtára nem található.");
        let home = crate::codex_account::home()?;
        let executable = crate::codex_runtime::resolve_executable()?;
        let mut command = native_account_command(&executable, &cwd, &home, &model, effort);
        crate::scoped_rules::configure_codex(&mut command, &cwd)?;
        if let Some(store) = &archive {
            store.configure(&mut command, true)?;
        }
        let lifetime = tempfile::Builder::new()
            .prefix("vibe-codex-native-")
            .tempdir()?;
        Self::spawn_command(command, lifetime, model, effort, repaint, archive)
    }

    pub fn spawn(
        cwd: PathBuf,
        model: String,
        effort: crate::app::Effort,
        token: String,
        repaint: egui::Context,
    ) -> Result<Self> {
        Self::spawn_persisted(cwd, model, effort, token, repaint, None)
    }
    pub fn spawn_persisted(
        cwd: PathBuf,
        model: String,
        effort: crate::app::Effort,
        token: String,
        repaint: egui::Context,
        archive: Option<Arc<TerminalStore>>,
    ) -> Result<Self> {
        ensure!(cwd.is_dir(), "A Codex projektkönyvtára nem található.");
        ensure!(!token.is_empty(), "VIBE ChatGPT bejelentkezés szükséges.");
        let home = tempfile::Builder::new()
            .prefix("vibe-codex-terminal-")
            .tempdir()
            .context("Codex CLI saját adatkönyvtára nem hozható létre")?;
        let executable = crate::codex_runtime::resolve_executable()?;
        let profile = archive
            .as_ref()
            .map(|store| store.native_home())
            .unwrap_or_else(|| home.path().to_owned());
        std::fs::create_dir_all(&profile)?;
        let mut command = interactive_command(&executable, &cwd, &profile, &model, effort, &token);
        crate::scoped_rules::configure_codex(&mut command, &cwd)?;
        if let Some(store) = &archive {
            store.configure(&mut command, true)?;
        }
        Self::spawn_command(command, home, model, effort, repaint, archive)
    }

    /// Launch the official CLI with its own native Google Sign-In/keyring state.
    pub fn spawn_antigravity(cwd: PathBuf, repaint: egui::Context) -> Result<Self> {
        Self::spawn_antigravity_persisted(cwd, repaint, None)
    }
    pub fn spawn_antigravity_persisted(
        cwd: PathBuf,
        repaint: egui::Context,
        archive: Option<Arc<TerminalStore>>,
    ) -> Result<Self> {
        ensure!(
            cwd.is_dir(),
            "Az Antigravity projektkönyvtára nem található."
        );
        let executable = crate::antigravity::resolve_executable()?;
        crate::scoped_rules::configure_antigravity(&cwd)?;
        let mut command = crate::antigravity::interactive_command(&executable, &cwd);
        if let Some(store) = &archive {
            store.configure(&mut command, false)?;
        }
        // The shared lifecycle owns a tempdir until exit. This is only a lifetime
        // guard: it never replaces AGY_HOME, HOME, or the CLI's native config.
        let lifetime = tempfile::Builder::new()
            .prefix("vibe-antigravity-terminal-")
            .tempdir()
            .context("Antigravity terminál munkakönyvtára nem hozható létre")?;
        Self::spawn_command(
            command,
            lifetime,
            "Antigravity".into(),
            crate::app::Effort::default(),
            repaint,
            archive,
        )
    }

    fn spawn_command(
        command: CommandBuilder,
        home: tempfile::TempDir,
        model: String,
        effort: crate::app::Effort,
        repaint: egui::Context,
        archive: Option<Arc<TerminalStore>>,
    ) -> Result<Self> {
        let cli_name = if model == "Antigravity" {
            "Antigravity CLI"
        } else {
            "Codex CLI"
        };
        let pair = native_pty_system()
            .openpty(pty_size(ROWS, COLS))
            .context("Natív terminál nem hozható létre")?;
        let reader = pair
            .master
            .try_clone_reader()
            .context("Terminál kimenete nem nyitható meg")?;
        let writer = pair.master.take_writer()?;
        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("Interaktív {cli_name} nem indítható"))?;
        let killer = child.clone_killer();
        let mut guard = ChildGuard {
            child,
            reaped: false,
        };
        drop(pair.slave);
        #[cfg(unix)]
        let process_group = pair.master.process_group_leader();
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            ROWS,
            COLS,
            SCROLLBACK,
            TerminalCallbacks::default(),
        )));
        let cached_screen = Arc::new(parser.lock().unwrap().screen().clone());
        let published = Arc::new(Mutex::new(cached_screen.clone()));
        let control = Arc::new(TerminalControl::new());
        let exit = Arc::new(Mutex::new(None));
        let (input, input_receiver) = mpsc::sync_channel(INPUT_QUEUE_CAPACITY);
        let (queries, query_receiver) = mpsc::sync_channel(QUERY_QUEUE_CAPACITY);
        let io_control = control.clone();
        let io_exit = exit.clone();
        let io_repaint = repaint.clone();
        thread::Builder::new()
            .name("vibe-terminal-input".into())
            .spawn(move || {
                run_io(
                    pair.master,
                    writer,
                    input_receiver,
                    query_receiver,
                    io_control,
                    io_exit,
                    io_repaint,
                )
            })
            .context("Terminál írószála nem indítható")?;
        let screen_parser = parser.clone();
        let screen_control = control.clone();
        let screen_published = published.clone();
        let screen_repaint = repaint.clone();
        let screen_archive = archive.clone();
        thread::Builder::new()
            .name("vibe-terminal-screen".into())
            .spawn(move || {
                publish_screen(
                    screen_parser,
                    screen_published,
                    screen_control,
                    screen_repaint,
                    screen_archive,
                )
            })
            .context("Terminál képernyőszála nem indítható")?;
        let output_control = control.clone();
        let output_exit = exit.clone();
        let output_repaint = repaint.clone();
        thread::Builder::new()
            .name("vibe-terminal-output".into())
            .spawn(move || {
                read_output(
                    reader,
                    parser,
                    queries,
                    output_control,
                    output_exit,
                    output_repaint,
                )
            })
            .context("Terminál olvasószála nem indítható")?;
        let wait_exit = exit.clone();
        let wait_repaint = repaint.clone();
        thread::Builder::new()
            .name("vibe-terminal-child".into())
            .spawn(move || {
                let result = guard.child.wait();
                guard.reaped = result.is_ok();
                drop(guard);
                let message = match result {
                    Ok(status) if status.success() => format!("{cli_name} befejeződött."),
                    Ok(status) => format!("{cli_name} leállt: {status}"),
                    Err(_) => format!("{cli_name} folyamatállapota nem olvasható."),
                };
                *wait_exit.lock().unwrap_or_else(|error| error.into_inner()) = Some(message);
                // Keep the Codex home or Antigravity lifetime guard until exit.
                drop(home);
                wait_repaint.request_repaint();
            })
            .context("Terminál folyamatfigyelője nem indítható")?;
        Ok(Self {
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            input: Some(input),
            model,
            effort,
            control,
            published,
            cached_screen,
            exit,
            killer: Some(killer),
            #[cfg(unix)]
            process_group,
            repaint,
            saved_screen: None,
            archive,
        })
    }

    pub fn restored(
        archive: TerminalArchive,
        store: Arc<TerminalStore>,
        model: String,
        effort: crate::app::Effort,
        repaint: egui::Context,
    ) -> Result<Self> {
        let saved = archive.replay()?;
        let screen = Arc::new(saved.clone());
        Ok(Self {
            instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
            input: None,
            model,
            effort,
            control: Arc::new(TerminalControl::new()),
            published: Arc::new(Mutex::new(screen.clone())),
            cached_screen: screen,
            exit: Arc::new(Mutex::new(Some(
                "Mentett CLI · Folytatás a fejlécben".into(),
            ))),
            killer: None,
            #[cfg(unix)]
            process_group: None,
            repaint,
            saved_screen: Some(Box::new(Mutex::new(saved))),
            archive: Some(store),
        })
    }
    pub fn is_restored(&self) -> bool {
        self.saved_screen.is_some()
    }
    pub fn save_and_stop(&mut self) -> Result<()> {
        let snapshot = if self.is_restored() {
            None
        } else {
            self.archive.clone().map(|store| {
                let screen = self
                    .published
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                (store, screen)
            })
        };
        self.stop();
        if let Some((store, screen)) = snapshot {
            store.save(&screen)?;
        }
        Ok(())
    }

    /// UI reads only a published pointer. Screen/history cloning happens off-thread.
    pub fn snapshot(&mut self) -> Arc<vt100::Screen> {
        if let Ok(screen) = self.published.try_lock() {
            self.cached_screen = screen.clone();
        }
        self.cached_screen.clone()
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        ensure!(
            !self.control.stopped.load(Ordering::Acquire) && self.exited().is_none(),
            "CLI már leállt."
        );
        ensure!(
            bytes.len() <= MAX_WRITE_BYTES,
            "CLI bemenet túl nagy (legfeljebb 64 KiB)."
        );
        let input = self.input.as_ref().context("CLI terminálja bezárva.")?;
        match input.try_send(bytes.to_vec()) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                anyhow::bail!("CLI bemeneti sora megtelt; próbáld újra.")
            }
            Err(TrySendError::Disconnected(_)) => anyhow::bail!("CLI terminálja bezárva."),
        }
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        if let Some(saved) = &self.saved_screen {
            let mut screen = saved.lock().unwrap_or_else(|error| error.into_inner());
            if screen.size() != (rows, cols) {
                screen.set_size(rows, cols);
                *self
                    .published
                    .lock()
                    .unwrap_or_else(|error| error.into_inner()) = Arc::new(screen.clone());
            }
            return Ok(());
        }

        ensure!(
            !self.control.stopped.load(Ordering::Acquire),
            "CLI terminálja bezárva."
        );
        // One atomic slot coalesces all pending resizes, including repeated frames.
        self.control.size.store(
            pack_size(rows.clamp(1, 1000), cols.clamp(1, 1000)),
            Ordering::Release,
        );
        Ok(())
    }

    /// Positive lines move up into history; negative lines return to live output.
    pub fn scroll(&self, lines: isize) {
        if let Some(saved) = &self.saved_screen {
            let mut screen = saved.lock().unwrap_or_else(|error| error.into_inner());
            let offset = screen.scrollback().saturating_add_signed(lines);
            screen.set_scrollback(offset);
            *self
                .published
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Arc::new(screen.clone());
            return;
        }

        let mut pending = self.control.scroll.load(Ordering::Acquire);
        while let Err(current) = self.control.scroll.compare_exchange_weak(
            pending,
            pending.saturating_add(lines),
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            pending = current;
        }
    }

    pub fn output_revision(&self) -> u64 {
        self.control.revision.load(Ordering::Acquire)
    }

    pub fn exited(&self) -> Option<String> {
        // Never let a status update pause the UI either.
        self.exit.try_lock().ok().and_then(|exit| exit.clone())
    }

    pub fn stop(&mut self) {
        if self.control.stopped.swap(true, Ordering::AcqRel) {
            return;
        }
        // The killer has no IO-worker mutex. A blocked ConPTY resize/write must
        // never prevent process termination. Master/writer close on their owner.
        #[cfg(unix)]
        if self.killer.is_some()
            && let Some(group) = self.process_group.filter(|group| *group > 1)
        {
            unsafe {
                libc::kill(-group, libc::SIGKILL);
            }
        }
        if let Some(mut killer) = self.killer.take() {
            let _ = killer.kill();
        }
        self.input.take();
        self.exit
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get_or_insert_with(|| "CLI panel bezárva.".into());
        self.repaint.request_repaint();
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ChildGuard {
    child: Box<dyn Child + Send + Sync>,
    reaped: bool,
}
impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !self.reaped {
            let _ = self.child.kill();
        }
    }
}

fn pty_size(rows: u16, cols: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn native_account_command(
    executable: &Path,
    cwd: &Path,
    home: &Path,
    model: &str,
    effort: crate::app::Effort,
) -> CommandBuilder {
    let mut command = CommandBuilder::new(executable);
    crate::terminal_persistence::clear_hook_environment(&mut command);
    command.args([
        "--no-daemon",
        "--no-alt-screen",
        "--model",
        model,
        "-c",
        "cli_auth_credentials_store=\"file\"",
        "-c",
        "model_provider=\"openai\"",
    ]);
    command.args([
        "-c",
        &format!("model_reasoning_effort=\"{}\"", effort.reasoning()),
    ]);
    command.cwd(cwd);
    command.env("CODEX_HOME", home);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    for name in crate::codex_account::CREDENTIAL_ENV {
        command.env_remove(name);
    }
    command
}

fn interactive_command(
    executable: &Path,
    cwd: &Path,
    home: &Path,
    model: &str,
    effort: crate::app::Effort,
    token: &str,
) -> CommandBuilder {
    let mut command = CommandBuilder::new(executable);
    crate::terminal_persistence::clear_hook_environment(&mut command);
    // --no-daemon prevents attachment to another Codex account's shared server.
    command.args(["--no-daemon", "--no-alt-screen", "--model", model]);
    command.args([
        "-c",
        &format!("model_reasoning_effort=\"{}\"", effort.reasoning()),
    ]);
    for setting in [
        "model_provider=\"openai_chatgpt_plan\"",
        "model_providers.openai_chatgpt_plan.name=\"ChatGPT plan\"",
        "model_providers.openai_chatgpt_plan.base_url=\"https://api.openai.com/v1\"",
        "model_providers.openai_chatgpt_plan.env_key=\"ACCESS_TOKEN\"",
        "model_providers.openai_chatgpt_plan.wire_api=\"responses\"",
        "model_providers.openai_chatgpt_plan.requires_openai_auth=false",
        "model_providers.openai_chatgpt_plan.supports_websockets=false",
        "shell_environment_policy.exclude=[\"ACCESS_TOKEN\"]",
    ] {
        command.args(["-c", setting]);
    }
    command.cwd(cwd);
    command.env("CODEX_HOME", home);
    command.env("ACCESS_TOKEN", token);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    for name in [
        "OPENAI_API_KEY",
        "OPENAI_ACCESS_TOKEN",
        "CODEX_API_KEY",
        "OPENROUTER_API_KEY",
        "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
        "CODEX_THREAD_ID",
        "CODEX_SANDBOX",
        "CODEX_SANDBOX_NETWORK_DISABLED",
    ] {
        command.env_remove(name);
    }
    command
}

/// Sole owner of the PTY control handle and input pipe. These OS calls can
/// block indefinitely on ConPTY; no UI method waits for this worker or joins it.
fn run_io(
    master: Box<dyn MasterPty + Send>,
    mut writer: Box<dyn Write + Send>,
    input: Receiver<Vec<u8>>,
    queries: Receiver<Vec<u8>>,
    control: Arc<TerminalControl>,
    exit: Exit,
    repaint: egui::Context,
) {
    let mut applied_size = pack_size(ROWS, COLS);
    while !control.stopped.load(Ordering::Acquire) {
        let requested_size = control.size.load(Ordering::Acquire);
        if requested_size != applied_size {
            let (rows, cols) = unpack_size(requested_size);
            if master.resize(pty_size(rows, cols)).is_err() {
                set_exit(&exit, "Terminál nem méretezhető át.", &repaint);
                break;
            }
            applied_size = requested_size;
        }
        // Queries get their own bounded lane, so a burst of user input cannot
        // make the output reader block while replying to terminal discovery.
        let bytes = match queries.try_recv() {
            Ok(bytes) => Some(bytes),
            Err(_) => match input.recv_timeout(WORKER_INTERVAL) {
                Ok(bytes) => Some(bytes),
                Err(mpsc::RecvTimeoutError::Timeout) => None,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            },
        };
        if let Some(bytes) = bytes
            && writer
                .write_all(&bytes)
                .and_then(|()| writer.flush())
                .is_err()
        {
            set_exit(&exit, "CLI bemeneti hiba.", &repaint);
            break;
        }
    }
    // ClosePseudoConsole may block too; both handles stay on this worker.
    drop(writer);
    drop(master);
}

fn publish_screen(
    parser: Parser,
    published: PublishedScreen,
    control: Arc<TerminalControl>,
    repaint: egui::Context,
    archive: Option<Arc<TerminalStore>>,
) {
    let mut last_save = std::time::Instant::now();
    let mut saved_revision = 0;
    let mut revision = 0;
    let mut size = pack_size(ROWS, COLS);
    while !control.stopped.load(Ordering::Acquire) {
        let next_revision = control.revision.load(Ordering::Acquire);
        let next_size = control.size.load(Ordering::Acquire);
        let scroll = control.scroll.swap(0, Ordering::AcqRel);
        if next_revision != revision || next_size != size || scroll != 0 {
            let screen = {
                let mut parser = parser.lock().unwrap_or_else(|error| error.into_inner());
                if next_size != size {
                    let (rows, cols) = unpack_size(next_size);
                    parser.screen_mut().set_size(rows, cols);
                }
                if scroll != 0 {
                    let offset = parser.screen().scrollback().saturating_add_signed(scroll);
                    parser.screen_mut().set_scrollback(offset);
                }
                Arc::new(parser.screen().clone())
            };
            // Drop old screens/history outside the pointer-publication lock.
            let old = std::mem::replace(
                &mut *published.lock().unwrap_or_else(|error| error.into_inner()),
                screen,
            );
            drop(old);
            revision = next_revision;
            size = next_size;
            repaint.request_repaint();
        }
        if let Some(store) = &archive
            && revision != saved_revision
            && last_save.elapsed() >= Duration::from_secs(2)
        {
            let screen = published
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone();
            let _ = store.save(&screen);
            saved_revision = revision;
            last_save = std::time::Instant::now();
        }
        thread::sleep(WORKER_INTERVAL);
    }
}

fn set_exit(exit: &Exit, message: &str, repaint: &egui::Context) {
    exit.lock()
        .unwrap_or_else(|error| error.into_inner())
        .get_or_insert_with(|| message.into());
    repaint.request_repaint();
}

fn read_output(
    mut reader: Box<dyn Read + Send>,
    parser: Parser,
    queries: SyncSender<Vec<u8>>,
    control: Arc<TerminalControl>,
    exit: Exit,
    repaint: egui::Context,
) {
    let mut buffer = [0; 16 * 1024];
    loop {
        let length = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(length) => length,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let responses = {
            let mut parser = parser.lock().unwrap_or_else(|error| error.into_inner());
            parser.process(&buffer[..length]);
            std::mem::take(&mut parser.callbacks_mut().responses)
        };
        control.revision.fetch_add(1, Ordering::Release);
        if !responses.is_empty() {
            // An unresponsive input pipe cannot stop output draining. Bound
            // memory even if a hostile child floods terminal queries.
            for response in responses.chunks(MAX_WRITE_BYTES) {
                let _ = queries.try_send(response.to_vec());
            }
        }
    }
    set_exit(&exit, "CLI terminálkapcsolata lezárult.", &repaint);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Cursor, sync::Condvar, time::Instant};

    #[test]
    fn dictation_stays_with_original_process_and_never_sends_enter() {
        let (first_tx, first_rx) = mpsc::sync_channel(2);
        let (_second_tx, second_rx) = mpsc::sync_channel::<Vec<u8>>(2);
        let control = Arc::new(TerminalControl::new());
        let target = DictationTarget {
            input: first_tx,
            control: control.clone(),
            exit: Arc::new(Mutex::new(None)),
        };
        target.insert("Készíts\r\nmagyar felületet\x1b").unwrap();
        assert_eq!(
            first_rx.try_recv().unwrap(),
            "Készíts magyar felületet".as_bytes()
        );
        assert!(second_rx.try_recv().is_err());
        control.stopped.store(true, Ordering::Release);
        assert!(target.insert("Ez már nem mehet sehova").is_err());
        assert!(first_rx.try_recv().is_err());
        assert!(second_rx.try_recv().is_err());
    }

    #[test]
    fn dictation_reports_full_queue_and_exited_process() {
        let (input, receiver) = mpsc::sync_channel(1);
        let exit = Arc::new(Mutex::new(None));
        let target = DictationTarget {
            input,
            control: Arc::new(TerminalControl::new()),
            exit: exit.clone(),
        };
        target.insert("első").unwrap();
        assert!(target.insert("második").is_err());
        receiver.try_recv().unwrap();
        *exit.lock().unwrap() = Some("kilépett".into());
        assert!(target.insert("harmadik").is_err());
        assert!(receiver.try_recv().is_err());
    }

    #[derive(Default)]
    struct BlockingGate {
        entered: AtomicBool,
        released: Mutex<bool>,
        wake: Condvar,
    }

    impl BlockingGate {
        fn block(&self) {
            self.entered.store(true, Ordering::Release);
            let mut released = self.released.lock().unwrap();
            while !*released {
                released = self.wake.wait(released).unwrap();
            }
        }

        fn wait_until_blocked(&self) {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !self.entered.load(Ordering::Acquire) {
                assert!(
                    Instant::now() < deadline,
                    "IO worker did not reach blocking call"
                );
                thread::sleep(Duration::from_millis(1));
            }
        }

        fn release(&self) {
            *self.released.lock().unwrap() = true;
            self.wake.notify_all();
        }
    }

    struct FakeMaster {
        resize_gate: Option<Arc<BlockingGate>>,
    }

    impl MasterPty for FakeMaster {
        fn resize(&self, _: PtySize) -> Result<()> {
            if let Some(gate) = &self.resize_gate {
                gate.block();
            }
            Ok(())
        }
        fn get_size(&self) -> Result<PtySize> {
            Ok(pty_size(ROWS, COLS))
        }
        fn try_clone_reader(&self) -> Result<Box<dyn Read + Send>> {
            Ok(Box::new(Cursor::new(Vec::<u8>::new())))
        }
        fn take_writer(&self) -> Result<Box<dyn Write + Send>> {
            Ok(Box::new(std::io::sink()))
        }
        #[cfg(unix)]
        fn process_group_leader(&self) -> Option<libc::pid_t> {
            None
        }
        #[cfg(unix)]
        fn as_raw_fd(&self) -> Option<portable_pty::unix::RawFd> {
            None
        }
        #[cfg(unix)]
        fn tty_name(&self) -> Option<PathBuf> {
            None
        }
    }

    struct FakeWriter(Option<Arc<BlockingGate>>);
    impl Write for FakeWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if let Some(gate) = &self.0 {
                gate.block();
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct FakeKiller(Arc<AtomicBool>);
    impl ChildKiller for FakeKiller {
        fn kill(&mut self) -> std::io::Result<()> {
            self.0.store(true, Ordering::Release);
            Ok(())
        }
        fn clone_killer(&self) -> Box<dyn ChildKiller + Send + Sync> {
            Box::new(Self(self.0.clone()))
        }
    }

    fn fake_session(
        resize_gate: Option<Arc<BlockingGate>>,
        write_gate: Option<Arc<BlockingGate>>,
    ) -> (TerminalSession, thread::JoinHandle<()>, Arc<AtomicBool>) {
        let parser = vt100::Parser::new(ROWS, COLS, 0);
        let screen = Arc::new(parser.screen().clone());
        let control = Arc::new(TerminalControl::new());
        let exit = Arc::new(Mutex::new(None));
        let (input, receiver) = mpsc::sync_channel(INPUT_QUEUE_CAPACITY);
        let (_queries, query_receiver) = mpsc::sync_channel(QUERY_QUEUE_CAPACITY);
        let repaint = egui::Context::default();
        let io_control = control.clone();
        let io_exit = exit.clone();
        let io_repaint = repaint.clone();
        let worker = thread::spawn(move || {
            run_io(
                Box::new(FakeMaster { resize_gate }),
                Box::new(FakeWriter(write_gate)),
                receiver,
                query_receiver,
                io_control,
                io_exit,
                io_repaint,
            )
        });
        let killed = Arc::new(AtomicBool::new(false));
        (
            TerminalSession {
                instance: NEXT_INSTANCE.fetch_add(1, Ordering::Relaxed),
                input: Some(input),
                model: "test-model".into(),
                effort: crate::app::Effort::default(),
                control,
                published: Arc::new(Mutex::new(screen.clone())),
                cached_screen: screen,
                exit,
                killer: Some(Box::new(FakeKiller(killed.clone()))),
                #[cfg(unix)]
                process_group: None,
                repaint,
                saved_screen: None,
                archive: None,
            },
            worker,
            killed,
        )
    }

    fn assert_ui_calls_do_not_wait(session: &mut TerminalSession) {
        let start = Instant::now();
        for index in 0..100 {
            session.resize(20 + index, 40 + index).unwrap();
            // Full queues report backpressure rather than dropping input silently.
            let _ = session.write(b"x");
            session.scroll(1);
            assert_eq!(session.snapshot().size(), (ROWS, COLS));
            assert!(session.exited().is_none());
        }
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "UI calls waited on blocked PTY: {:?}",
            start.elapsed()
        );
    }

    #[test]
    fn focused_terminal_frames_do_not_reenter_the_context_lock() {
        let (done, completed) = mpsc::channel();
        let render = thread::spawn(move || {
            let (mut session, worker, _) = fake_session(None, None);
            let ctx = egui::Context::default();
            let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(640.0, 480.0));
            for frame in 0..4 {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(rect),
                        focused: true,
                        events: if frame == 2 {
                            vec![egui::Event::Text("/".into())]
                        } else {
                            Vec::new()
                        },
                        ..Default::default()
                    },
                    |ctx| {
                        ctx.memory_mut(|memory| {
                            memory.request_focus(egui::Id::new(("native_terminal", 42_u64)))
                        });
                        crate::terminal_widget::show(ctx, 42, rect, &mut session, true);
                    },
                );
            }
            session.stop();
            worker.join().unwrap();
            done.send(()).unwrap();
        });
        completed
            .recv_timeout(Duration::from_secs(3))
            .expect("focused terminal frame deadlocked inside egui Context");
        render.join().unwrap();
    }

    #[test]
    fn blocked_conpty_resize_does_not_block_ui_or_kill() {
        let gate = Arc::new(BlockingGate::default());
        let (mut session, worker, killed) = fake_session(Some(gate.clone()), None);
        session.resize(12, 24).unwrap();
        gate.wait_until_blocked();
        assert_ui_calls_do_not_wait(&mut session);
        let start = Instant::now();
        session.stop();
        assert!(start.elapsed() < Duration::from_millis(50));
        assert!(killed.load(Ordering::Acquire));
        gate.release();
        worker.join().unwrap();
    }

    #[test]
    fn blocked_input_pipe_does_not_block_ui_or_kill() {
        let gate = Arc::new(BlockingGate::default());
        let (mut session, worker, killed) = fake_session(None, Some(gate.clone()));
        session.write(b"start").unwrap();
        gate.wait_until_blocked();
        assert_ui_calls_do_not_wait(&mut session);
        assert!(session.write(b"queue is full").is_err());
        assert!(session.write(&vec![0; MAX_WRITE_BYTES + 1]).is_err());
        session.stop();
        assert!(killed.load(Ordering::Acquire));
        gate.release();
        worker.join().unwrap();
    }

    #[test]
    fn snapshot_reuses_previous_screen_when_publication_is_busy() {
        let (mut session, worker, _) = fake_session(None, None);
        let previous = session.snapshot();
        let published = session.published.clone();
        let _busy = published.lock().unwrap();
        let start = Instant::now();
        for _ in 0..100 {
            assert!(Arc::ptr_eq(&previous, &session.snapshot()));
        }
        assert!(start.elapsed() < Duration::from_millis(50));
        session.stop();
        worker.join().unwrap();
    }

    #[test]
    fn output_keeps_draining_when_query_response_queue_is_full() {
        let parser = Arc::new(Mutex::new(vt100::Parser::new_with_callbacks(
            ROWS,
            COLS,
            SCROLLBACK,
            TerminalCallbacks::default(),
        )));
        let (queries, _receiver) = mpsc::sync_channel(1);
        queries.try_send(b"occupied".to_vec()).unwrap();
        let mut output = b"\x1b[6n".repeat(10_000);
        output.extend_from_slice(b"reader reached end");
        let observed = parser.clone();
        let (done, completed) = mpsc::channel();
        thread::spawn(move || {
            read_output(
                Box::new(Cursor::new(output)),
                parser,
                queries,
                Arc::new(TerminalControl::new()),
                Arc::new(Mutex::new(None)),
                egui::Context::default(),
            );
            done.send(()).unwrap();
        });
        completed
            .recv_timeout(Duration::from_secs(2))
            .expect("output reader waited on the blocked query-response pipe");
        assert!(
            observed
                .lock()
                .unwrap()
                .screen()
                .contents()
                .contains("reader reached end")
        );
    }

    #[test]
    fn terminal_queries_keep_cursor_colors_and_screen_updates() {
        let mut parser =
            vt100::Parser::new_with_callbacks(12, 40, 100, TerminalCallbacks::default());
        // Split sequences exactly as PTY reads can split them.
        parser.process(b"\x1b[3;8Hhello\x1b[");
        parser.process(b"6n\x1b[c\x1b[>c\x1b]11;");
        parser.process(b"?\x1b\\\x1b[?2026$p\x1b[18t");
        let responses = String::from_utf8(parser.callbacks().responses.clone()).unwrap();
        assert!(responses.contains("\x1b[3;13R"));
        assert!(responses.contains("\x1b[?1;2c"));
        assert!(responses.contains("\x1b[>0;1;0c"));
        assert!(responses.contains("\x1b]11;rgb:"));
        assert!(responses.contains("\x1b[?2026;0$y"));
        assert!(responses.contains("\x1b[8;12;40t"));
        assert!(parser.screen().contents().contains("hello"));
    }

    #[test]
    fn native_terminal_uses_usage_account_profile_without_inherited_credentials() {
        let command = native_account_command(
            Path::new("codex"),
            Path::new("project"),
            Path::new("vibe-native-profile"),
            "test-model",
            crate::app::Effort::Fast,
        );
        let arguments: Vec<_> = command
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert!(
            arguments
                .iter()
                .any(|arg| *arg == crate::codex_account::CREDENTIAL_STORE_SETTING)
        );
        assert!(
            arguments
                .iter()
                .any(|arg| *arg == "model_provider=\"openai\"")
        );
        assert_eq!(
            command.get_env("CODEX_HOME"),
            Some(std::ffi::OsStr::new("vibe-native-profile"))
        );
        for name in crate::codex_account::CREDENTIAL_ENV {
            assert!(command.get_env(name).is_none(), "inherited {name}");
        }
        assert!(
            !arguments
                .iter()
                .any(|arg| arg.contains("openai_chatgpt_plan"))
        );
    }

    #[test]
    fn interactive_command_has_no_prompt_server_or_approval_bypass() {
        let command = interactive_command(
            Path::new("codex"),
            Path::new("."),
            Path::new("isolated"),
            "test-model",
            crate::app::Effort::default(),
            "synthetic-token",
        );
        let arguments: Vec<_> = command
            .get_argv()
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect();
        assert!(!arguments.iter().any(|arg| matches!(
            arg.as_ref(),
            "--ask-for-approval" | "--sandbox"
        )
            || arg.starts_with("sandbox_workspace_write.network_access=")));
        assert!(arguments.iter().any(|arg| *arg == "--no-daemon"));
        assert!(
            arguments
                .iter()
                .any(|arg| *arg == "model_reasoning_effort=\"low\"")
        );
        assert!(
            !arguments
                .iter()
                .any(|arg| *arg == "--model-reasoning-effort")
        );
        assert!(!arguments.iter().any(|arg| arg.contains("app-server")
            || arg.contains("synthetic-token")
            || arg.contains("dangerously")));
    }

    /// Manual integration smoke only: no real account, no prompt, no inference.
    #[test]
    #[ignore = "Requires installed Codex CLI; isolated profile and synthetic token, no inference"]
    fn installed_cli_permission_changes_are_session_scoped() {
        let directory = tempfile::tempdir().unwrap();
        let profile = directory.path().join("profile");
        std::fs::create_dir_all(&profile).unwrap();
        std::fs::write(
            profile.join("config.toml"),
            "sandbox_mode = \"read-only\"\napproval_policy = \"on-request\"\n",
        )
        .unwrap();
        let executable = crate::codex_runtime::resolve_executable().unwrap();
        let command = interactive_command(
            &executable,
            directory.path(),
            &profile,
            "gpt-6.1-sol",
            crate::app::Effort::default(),
            "vibe-synthetic-not-a-real-token",
        );
        let mut session = TerminalSession::spawn_command(
            command,
            tempfile::tempdir().unwrap(),
            "gpt-6.1-sol".into(),
            crate::app::Effort::default(),
            egui::Context::default(),
            None,
        )
        .unwrap();
        let deadline = Instant::now() + Duration::from_secs(15);
        while !session.snapshot().contents().contains("OpenAI Codex") {
            assert!(session.exited().is_none());
            assert!(Instant::now() < deadline);
            thread::sleep(Duration::from_millis(50));
        }
        session.write(b"/permissions").unwrap();
        thread::sleep(Duration::from_millis(200));
        session.write(b"\r").unwrap();
        thread::sleep(Duration::from_secs(1));
        assert!(
            session
                .snapshot()
                .contents()
                .contains("Update Model Permissions")
        );
        session.write(b"3").unwrap();
        thread::sleep(Duration::from_millis(200));
        session.write(b"\r").unwrap();
        thread::sleep(Duration::from_secs(1));
        session.write(b"/permissions").unwrap();
        thread::sleep(Duration::from_millis(200));
        session.write(b"\r").unwrap();
        thread::sleep(Duration::from_secs(1));
        let screen = session.snapshot().contents();
        assert!(screen.contains("Full Access (current)"), "{screen}");
        let config = std::fs::read_to_string(profile.join("config.toml")).unwrap();
        assert!(config.contains("sandbox_mode = \"read-only\""));
        session.stop();
    }

    /// Manual integration smoke only: no real account, no prompt, no inference.
    #[test]
    #[ignore = "Requires an installed interactive Codex CLI and native PTY"]
    fn installed_cli_opens_original_tui_with_synthetic_credentials() {
        let cwd = tempfile::tempdir().unwrap();
        let mut session = TerminalSession::spawn(
            cwd.path().into(),
            "gpt-6.1-sol".into(),
            crate::app::Effort::default(),
            "vibe-synthetic-smoke-token-not-a-real-credential".into(),
            egui::Context::default(),
        )
        .unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let contents = session.snapshot().contents();
            if contents.contains("OpenAI Codex") {
                break;
            }
            assert!(
                session.exited().is_none(),
                "CLI exited before TUI: {contents}"
            );
            assert!(
                std::time::Instant::now() < deadline,
                "Original TUI header missing: {contents}"
            );
            thread::sleep(std::time::Duration::from_millis(50));
        }
        session.resize(35, 110).unwrap();
        session.write(b"/").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(4);
        loop {
            let contents = session.snapshot().contents();
            if contents.contains("/model") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "Slash menu missing: {contents}"
            );
            thread::sleep(std::time::Duration::from_millis(50));
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while session.snapshot().size() != (35, 110) {
            assert!(
                std::time::Instant::now() < deadline,
                "Resize was not published"
            );
            thread::sleep(std::time::Duration::from_millis(10));
        }
        session.stop();
        assert!(session.exited().is_some());
    }
}
