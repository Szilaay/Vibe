//! On-demand project browser and local, bounded Git inspection.
use crate::workspace_tools::{Directory, Workspace};
use anyhow::{Context, Result, ensure};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone)]
pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(String),
}
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Request {
    Directory(String),
    File(String),
    Changes,
    Diff(String),
}
#[derive(Debug, Clone)]
pub struct GitChange {
    pub path: String,
    pub status: String,
}
#[derive(Debug, Clone)]
pub struct Changes {
    pub entries: Vec<GitChange>,
    pub truncated: bool,
}
#[derive(Debug, Clone)]
pub enum Response {
    Directory(Directory),
    Text(String),
    Changes(Changes),
}
#[derive(Debug, Default)]
pub struct Browser {
    pub directories: HashMap<String, Load<Directory>>,
    pub changes: Option<Load<Changes>>,
    pub preview: Option<(Request, Load<String>)>,
}
impl Browser {
    pub fn begin(&mut self, request: &Request) {
        match request {
            Request::Directory(path) => {
                self.directories.insert(path.clone(), Load::Loading);
            }
            Request::Changes => self.changes = Some(Load::Loading),
            _ => self.preview = Some((request.clone(), Load::Loading)),
        }
    }
    pub fn complete(&mut self, request: &Request, result: Result<Response, String>) {
        match request {
            Request::Directory(path) => {
                self.directories.insert(
                    path.clone(),
                    match result {
                        Ok(Response::Directory(data)) => Load::Ready(data),
                        Err(error) => Load::Failed(error),
                        _ => Load::Failed("Hibás mappaválasz.".into()),
                    },
                );
            }
            Request::Changes => {
                self.changes = Some(match result {
                    Ok(Response::Changes(data)) => Load::Ready(data),
                    Err(error) => Load::Failed(error),
                    _ => Load::Failed("Hibás Git-válasz.".into()),
                })
            }
            _ if self
                .preview
                .as_ref()
                .is_some_and(|(current, _)| current == request) =>
            {
                self.preview = Some((
                    request.clone(),
                    match result {
                        Ok(Response::Text(text)) => Load::Ready(text),
                        Err(error) => Load::Failed(error),
                        _ => Load::Failed("Hibás fájlválasz.".into()),
                    },
                ));
            }
            _ => (),
        }
    }
}

pub async fn load(root: PathBuf, request: Request) -> Result<Response> {
    match request {
        Request::Directory(path) => {
            tokio::task::spawn_blocking(move || {
                Workspace::new(&root)?
                    .list_directory(&path)
                    .map(Response::Directory)
            })
            .await?
        }
        Request::File(path) => {
            tokio::task::spawn_blocking(move || {
                Workspace::new(&root)?.read_text(&path).map(Response::Text)
            })
            .await?
        }
        Request::Changes => changes(&root).await.map(Response::Changes),
        Request::Diff(path) => diff(&root, &path).await.map(Response::Text),
    }
}

fn executable() -> PathBuf {
    #[cfg(windows)]
    if let Some(programs) = std::env::var_os("ProgramFiles") {
        let path = PathBuf::from(programs).join("Git/cmd/git.exe");
        if path.is_file() {
            return path;
        }
    }
    PathBuf::from("git")
}

async fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    // Git status/diff may otherwise start clean filters (for example git-lfs).
    // Read effective driver names without executing them, then disable each one.
    let config = git_raw(
        root,
        &[
            "config",
            "--null",
            "--name-only",
            "--get-regexp",
            r"^filter\..*\.(clean|smudge|process|required)$",
        ],
        true,
    )
    .await?;
    let mut drivers = std::collections::BTreeSet::new();
    for name in config.split(|b| *b == 0).filter(|name| !name.is_empty()) {
        let name = std::str::from_utf8(name)?;
        let driver = name
            .rsplit_once('.')
            .context("Hibás Git filter-beállítás")?
            .0;
        ensure!(
            driver.len() <= 512 && !driver.contains('=') && !driver.chars().any(char::is_control),
            "Hibás Git filter-név."
        );
        drivers.insert(driver);
    }
    ensure!(drivers.len() <= 128, "Túl sok Git filter-beállítás.");
    let mut arguments = Vec::new();
    for driver in drivers {
        for suffix in ["clean=", "smudge=", "process=", "required=false"] {
            arguments.push("-c".to_owned());
            arguments.push(format!("{driver}.{suffix}"));
        }
    }
    arguments.extend(args.iter().map(|arg| (*arg).to_owned()));
    git_raw(
        root,
        &arguments.iter().map(String::as_str).collect::<Vec<_>>(),
        false,
    )
    .await
}

async fn git_raw(root: &Path, args: &[&str], allow_no_match: bool) -> Result<Vec<u8>> {
    let mut command = tokio::process::Command::new(executable());
    command
        .current_dir(root)
        .args([
            "--no-pager",
            "--no-optional-locks",
            "--literal-pathspecs",
            "-c",
            "core.fsmonitor=false",
            "-c",
            "color.ui=false",
        ])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    for key in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_EXTERNAL_DIFF",
    ] {
        command.env_remove(key);
    }
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .context("Git nem található vagy nem indítható.")?;
    let result = tokio::time::timeout(Duration::from_secs(15), async {
        let mut bytes = Vec::new();
        child.stdout.take().context("Hiányzó Git-kimenet")?.take(512*1024+1).read_to_end(&mut bytes).await?;
        ensure!(bytes.len() <= 512*1024, "A Git-kimenet nagyobb 512 KiB-nál; szűkítsd a workspace-t.");
        let status = child.wait().await?;
        ensure!(status.success() || allow_no_match && status.code()==Some(1), "A Git-művelet nem sikerült. Ellenőrizd, hogy a workspace Git-repó-e és nincs-e tulajdonosi vagy jogosultsági hiba.");
        Ok(bytes)
    }).await;
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        Ok(Err(error)) => {
            let _ = child.kill().await;
            Err(error)
        }
        Err(_) => {
            let _ = child.kill().await;
            anyhow::bail!("Git időtúllépés (15 s).")
        }
    }
}

async fn changes(root: &Path) -> Result<Changes> {
    let prefix = git(root, &["rev-parse", "--show-prefix"]).await?;
    let prefix = std::str::from_utf8(&prefix)?.trim_end_matches(['\r', '\n']);
    let bytes = git(
        root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--ignore-submodules=all",
            "--no-renames",
            "--",
            ".",
        ],
    )
    .await?;
    let workspace = Workspace::new(root)?;
    let mut changes = parse_status(&bytes, prefix)?;
    changes
        .entries
        .retain(|item| workspace.change_path(&item.path).is_ok());
    Ok(changes)
}

fn parse_status(bytes: &[u8], prefix: &str) -> Result<Changes> {
    let mut entries = Vec::new();
    let mut rows = bytes.split(|b| *b == 0).filter(|row| !row.is_empty());
    let mut truncated = false;
    while let Some(row) = rows.next() {
        ensure!(
            row.len() >= 4 && row[2] == b' ',
            "Hibás Git status formátum."
        );
        let status = std::str::from_utf8(&row[..2])?;
        let path = std::str::from_utf8(&row[3..])?;
        if status.contains(['R', 'C']) {
            let _ = rows.next().context("Hiányos Git-átnevezés")?;
        }
        let Some(path) = path.strip_prefix(prefix) else {
            continue;
        };
        if entries.len() == 500 {
            truncated = true;
            break;
        }
        entries.push(GitChange {
            path: path.into(),
            status: status.into(),
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(Changes { entries, truncated })
}

async fn diff(root: &Path, relative: &str) -> Result<String> {
    let workspace = Workspace::new(root)?;
    let path = workspace.change_path(relative)?;
    ensure!(!path.is_dir(), "A diffhez fájlt válassz, ne teljes mappát.");
    let staged = git(
        root,
        &[
            "diff",
            "--cached",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--",
            relative,
        ],
    )
    .await?;
    let working = git(
        root,
        &[
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--",
            relative,
        ],
    )
    .await?;
    let mut text = String::new();
    if !staged.is_empty() {
        text.push_str("# STAGED / INDEX\n");
        text.push_str(std::str::from_utf8(&staged).context("A diff nem UTF-8")?);
    }
    if !working.is_empty() {
        text.push_str("\n# WORKING TREE\n");
        text.push_str(std::str::from_utf8(&working).context("A diff nem UTF-8")?);
    }
    if text.is_empty() {
        let status = git(root, &["ls-files", "--error-unmatch", "--", relative]).await;
        if status.is_err() {
            let root = root.to_owned();
            let path = relative.to_owned();
            let content =
                tokio::task::spawn_blocking(move || Workspace::new(&root)?.read_text(&path))
                    .await??;
            text.push_str("# Új / nem követett fájl\n");
            for line in content.lines() {
                text.push('+');
                text.push_str(line);
                text.push('\n');
            }
        } else {
            text.push_str("Nincs szöveges eltérés az aktuális indexhez és munkafájlhoz képest.");
        }
    }
    Ok(crate::workspace_tools::bounded(text, 256 * 1024))
}

pub const CONTEXT_MARKER: &str = "\n\n<VIBE_CONTEXT_FILES>\n";
pub fn attach_context(root: &Path, paths: &[String], prompt: &str) -> Result<String> {
    ensure!(paths.len() <= 8, "Legfeljebb 8 fájl csatolható.");
    let workspace = Workspace::new(root)?;
    let mut files = Vec::new();
    let mut total = 0;
    for path in paths {
        let content = workspace.read_text(path)?;
        total += content.len();
        ensure!(
            total <= 64 * 1024,
            "A csatolmányok együtt legfeljebb 64 KiB méretűek lehetnek. A nagy fájlt az ügynök eszközzel olvassa."
        );
        files.push(serde_json::json!({"path":path,"content":content}));
    }
    Ok(format!(
        "{prompt}{CONTEXT_MARKER}{}\n</VIBE_CONTEXT_FILES>",
        serde_json::to_string_pretty(&files)?
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn real_git_tracks_staged_working_deleted_untracked_and_nested_paths_without_filters() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        git(root, &["-c", "init.templateDir=", "init"])
            .await
            .unwrap();
        git(root, &["config", "user.name", "VIBE Test"])
            .await
            .unwrap();
        git(root, &["config", "user.email", "test@example.invalid"])
            .await
            .unwrap();
        std::fs::create_dir(root.join("hooks-empty")).unwrap();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("a file.rs"), "original\n").unwrap();
        std::fs::write(root.join("delete.rs"), "deleted\n").unwrap();
        std::fs::write(root.join("src/sub.rs"), "nested original\n").unwrap();
        git(root, &["add", "a file.rs", "delete.rs", "src/sub.rs"])
            .await
            .unwrap();
        let hooks = format!("core.hooksPath={}", root.join("hooks-empty").display());
        git(
            root,
            &[
                "-c",
                &hooks,
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "fixture",
            ],
        )
        .await
        .unwrap();
        std::fs::write(root.join("a file.rs"), "staged\n").unwrap();
        git(root, &["add", "a file.rs"]).await.unwrap();
        std::fs::write(root.join("a file.rs"), "working\n").unwrap();
        std::fs::remove_file(root.join("delete.rs")).unwrap();
        std::fs::write(root.join("src/sub.rs"), "nested changed\n").unwrap();
        std::fs::write(root.join("új.rs"), "new file\n").unwrap();
        std::fs::write(root.join(".env"), "private-token").unwrap();
        // Any accidental external diff, textconv or clean filter would fail this test.
        git(root, &["config", "diff.external", "VIBE-DO-NOT-RUN"])
            .await
            .unwrap();
        git(
            root,
            &["config", "diff.fixture.textconv", "VIBE-DO-NOT-RUN"],
        )
        .await
        .unwrap();
        git(root, &["config", "filter.fixture.clean", "VIBE-DO-NOT-RUN"])
            .await
            .unwrap();
        git(
            root,
            &["config", "filter.fixture.process", "VIBE-DO-NOT-RUN"],
        )
        .await
        .unwrap();
        git(root, &["config", "filter.fixture.required", "true"])
            .await
            .unwrap();
        std::fs::write(
            root.join(".gitattributes"),
            "*.rs filter=fixture diff=fixture\n",
        )
        .unwrap();
        let status = changes(root).await.unwrap();
        assert!(
            status
                .entries
                .iter()
                .any(|c| c.path == "a file.rs" && c.status == "MM")
        );
        assert!(
            status
                .entries
                .iter()
                .any(|c| c.path == "delete.rs" && c.status.contains('D'))
        );
        assert!(!status.entries.iter().any(|c| c.path == ".env"));
        let text = diff(root, "a file.rs").await.unwrap();
        assert!(text.contains("STAGED / INDEX") && text.contains("WORKING TREE"));
        assert!(text.contains("-original") && text.contains("+working"));
        assert!(diff(root, "delete.rs").await.unwrap().contains("-deleted"));
        assert!(diff(root, "új.rs").await.unwrap().contains("+new file"));
        assert!(diff(root, ".env").await.is_err());
        let nested = changes(&root.join("src")).await.unwrap();
        assert_eq!(nested.entries.len(), 1);
        assert_eq!(nested.entries[0].path, "sub.rs");
    }
    #[test]
    fn nul_status_handles_spaces_unicode_renames_and_nested_workspaces() {
        let rows = b" M app/a file.rs\0R  app/new.rs\0app/old.rs\0?? outside.rs\0";
        let changes = parse_status(rows, "app/").unwrap();
        assert_eq!(changes.entries.len(), 2);
        assert_eq!(changes.entries[0].path, "a file.rs");
        assert_eq!(changes.entries[1].path, "new.rs");
        assert!(parse_status(b"garbage\0", "").is_err());
    }
    #[test]
    fn browser_ignores_late_preview_results_after_switch_or_close() {
        let mut browser = Browser::default();
        browser.begin(&Request::File("a".into()));
        browser.begin(&Request::File("b".into()));
        browser.complete(&Request::File("a".into()), Ok(Response::Text("old".into())));
        assert!(
            matches!(browser.preview,Some((Request::File(ref path),Load::Loading)) if path=="b")
        );
        browser.preview = None;
        browser.complete(
            &Request::File("b".into()),
            Ok(Response::Text("late".into())),
        );
        assert!(browser.preview.is_none());
    }
    #[test]
    fn attachments_are_actual_snapshots_bounded_and_workspace_scoped() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.rs"), "one").unwrap();
        let first = attach_context(dir.path(), &["a.rs".into()], "question").unwrap();
        std::fs::write(dir.path().join("a.rs"), "two").unwrap();
        let second = attach_context(dir.path(), &["a.rs".into()], "question").unwrap();
        assert!(first.contains("one") && second.contains("two"));
        assert!(attach_context(dir.path(), &["../outside".into()], "q").is_err());
        std::fs::write(dir.path().join("a.rs"), vec![b'x'; 65537]).unwrap();
        assert!(attach_context(dir.path(), &["a.rs".into()], "q").is_err());
    }
}
