//! Bounded project tools. Models never receive ambient filesystem or shell access.
use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use std::{
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};

const MAX_FILE: usize = 256 * 1024;
const MAX_RESULT: usize = 24 * 1024;

#[derive(Debug, Clone)]
pub struct EditProposal {
    pub path: String,
    pub before: Option<String>,
    pub after: String,
}

#[derive(Clone)]
pub struct Workspace {
    root: PathBuf,
}

pub enum Outcome {
    Text(String),
    Edit(EditProposal),
}

#[derive(Debug, Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub directory: bool,
}
#[derive(Debug, Clone)]
pub struct Directory {
    pub entries: Vec<FileEntry>,
    pub truncated: bool,
}

impl Workspace {
    pub fn list_directory(&self, relative: &str) -> Result<Directory> {
        let path = self.resolve(relative, false)?;
        ensure!(path.is_dir(), "Nem mappa.");
        let mut entries = Vec::new();
        let mut truncated = false;
        for (index, entry) in fs::read_dir(&path)?.enumerate() {
            if index >= 500 {
                truncated = true;
                break;
            }
            let entry = entry?;
            let kind = entry.file_type()?;
            let relative = entry
                .path()
                .strip_prefix(&self.root)?
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            if kind.is_symlink() || self.resolve(&relative, false).is_err() {
                continue;
            }
            if !kind.is_dir() && !kind.is_file() {
                continue;
            }
            entries.push(FileEntry {
                name: entry.file_name().to_string_lossy().into(),
                path: relative,
                directory: kind.is_dir(),
            });
        }
        entries.sort_by(|a, b| {
            b.directory
                .cmp(&a.directory)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(Directory { entries, truncated })
    }
    pub fn read_text(&self, relative: &str) -> Result<String> {
        self.read(&self.resolve(relative, false)?)
    }
    pub(crate) fn change_path(&self, relative: &str) -> Result<PathBuf> {
        // Deleted Git files may have a parent directory that no longer exists.
        ensure!(
            !relative.contains([':', '\0']),
            "Érvénytelen projektútvonal."
        );
        for part in Path::new(relative).components() {
            match part {
                Component::CurDir => (),
                Component::Normal(name) => {
                    let name = name.to_string_lossy().to_ascii_lowercase();
                    ensure!(
                        !blocked(&name) && !name.ends_with([' ', '.']),
                        "Védett projektútvonal."
                    );
                }
                _ => bail!("Csak workspace-en belüli relatív útvonal használható."),
            }
        }
        let mut existing = self.root.join(relative);
        let mut tail = Vec::new();
        while !existing.exists() {
            ensure!(
                !fs::symlink_metadata(&existing).is_ok_and(|meta| meta.file_type().is_symlink()),
                "Nem követhető szimbolikus link."
            );
            tail.push(existing.file_name().context("Hiányzó fájlnév")?.to_owned());
            ensure!(existing.pop(), "Az útvonal kilép a workspace-ből.");
        }
        let relative = existing
            .strip_prefix(&self.root)?
            .to_str()
            .context("Az útvonal nem UTF-8.")?;
        let mut resolved = self.resolve(relative, false)?;
        for name in tail.into_iter().rev() {
            resolved.push(name);
        }
        Ok(resolved)
    }
    pub fn new(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        ensure!(root.is_dir(), "A workspace nem mappa.");
        Ok(Self { root })
    }

    fn resolve(&self, relative: &str, create: bool) -> Result<PathBuf> {
        ensure!(
            !relative.contains([':', '\0']),
            "Érvénytelen projektútvonal."
        );
        let path = Path::new(relative);
        for part in path.components() {
            match part {
                Component::CurDir => (),
                Component::Normal(name) => {
                    let name = name.to_string_lossy().to_ascii_lowercase();
                    ensure!(
                        !blocked(&name) && !name.ends_with([' ', '.']),
                        "Ez az útvonal nem érhető el a projektügynöknek."
                    );
                }
                _ => bail!("Csak workspace-en belüli relatív útvonal használható."),
            }
        }
        let joined = self.root.join(path);
        let resolved = if create && !joined.exists() {
            joined
                .parent()
                .context("Hiányzó szülőmappa")?
                .canonicalize()?
                .join(joined.file_name().context("Hiányzó fájlnév")?)
        } else {
            joined.canonicalize()?
        };
        ensure!(
            resolved.starts_with(&self.root),
            "Az útvonal kilép a workspace-ből."
        );
        // Validate canonical components too: an innocently named link must not expose .git or .env.
        for part in resolved.strip_prefix(&self.root)?.components() {
            if let Component::Normal(name) = part {
                ensure!(
                    !blocked(&name.to_string_lossy().to_ascii_lowercase()),
                    "Védett projektfájl."
                );
            }
        }
        Ok(resolved)
    }

    fn read(&self, path: &Path) -> Result<String> {
        ensure!(path.is_file(), "Nem fájl.");
        let file = fs::File::open(path)?;
        ensure!(
            file.metadata()?.len() <= MAX_FILE as u64,
            "A fájl nagyobb 256 KiB-nál."
        );
        let mut bytes = Vec::new();
        file.take((MAX_FILE + 1) as u64).read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() <= MAX_FILE && !bytes.contains(&0),
            "Túl nagy vagy bináris fájl."
        );
        String::from_utf8(bytes).context("Csak UTF-8 szövegfájl olvasható.")
    }

    pub fn execute(&self, name: &str, args: &Value) -> Result<Outcome> {
        let relative = args.get("path").and_then(Value::as_str).unwrap_or(".");
        let path = self.resolve(relative, name == "write_file")?;
        let result = match name {
            "list_directory" => {
                ensure!(path.is_dir(), "Nem mappa.");
                let mut rows = Vec::new();
                for entry in fs::read_dir(&path)?.take(501) {
                    let entry = entry?;
                    let kind = entry.file_type()?;
                    let name = entry.file_name().to_string_lossy().into_owned();
                    if blocked(&name.to_ascii_lowercase()) || kind.is_symlink() {
                        continue;
                    }
                    rows.push(format!(
                        "{}{name}",
                        if kind.is_dir() { "[dir] " } else { "      " }
                    ));
                }
                rows.sort();
                format!(
                    "{relative}\n{}\nLegfeljebb 501 könyvtárbejegyzés vizsgálva.",
                    rows.join("\n")
                )
            }
            "read_file" => {
                let text = self.read(&path)?;
                let start = args
                    .get("start_line")
                    .and_then(Value::as_u64)
                    .unwrap_or(1)
                    .max(1) as usize;
                let count = args
                    .get("line_count")
                    .and_then(Value::as_u64)
                    .unwrap_or(200)
                    .clamp(1, 500) as usize;
                text.lines()
                    .enumerate()
                    .skip(start - 1)
                    .take(count)
                    .map(|(i, line)| format!("{} | {line}\n", i + 1))
                    .collect()
            }
            "search_files" => {
                let query = args
                    .get("query")
                    .and_then(Value::as_str)
                    .context("Hiányzó keresőkifejezés")?;
                ensure!(
                    !query.is_empty() && query.len() <= 256,
                    "1–256 bájtos keresőkifejezés kell."
                );
                self.search(&path, query)?
            }
            "write_file" => {
                let after = args
                    .get("content")
                    .and_then(Value::as_str)
                    .context("Hiányzó fájltartalom")?;
                ensure!(
                    after.len() <= MAX_FILE && !after.contains('\0'),
                    "Csak legfeljebb 256 KiB szöveg írható."
                );
                let before = if path.exists() {
                    Some(self.read(&path)?)
                } else {
                    None
                };
                return Ok(Outcome::Edit(EditProposal {
                    path: relative.into(),
                    before,
                    after: after.into(),
                }));
            }
            _ => bail!("Ismeretlen projekteszköz."),
        };
        Ok(Outcome::Text(bounded(result, MAX_RESULT)))
    }

    fn search(&self, start: &Path, query: &str) -> Result<String> {
        let mut pending = vec![start.to_owned()];
        let mut output = String::new();
        let mut visited = 0;
        let mut bytes = 0;
        while let Some(path) = pending.pop() {
            visited += 1;
            if visited > 2000 || bytes >= 8 * 1024 * 1024 || output.len() >= MAX_RESULT {
                break;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            if meta.file_type().is_symlink() {
                continue;
            }
            let relative = path.strip_prefix(&self.root)?.to_string_lossy();
            let Ok(path) = self.resolve(&relative, false) else {
                continue;
            };
            if meta.is_dir() {
                for entry in fs::read_dir(path)?.take(2000 - visited).flatten() {
                    if pending.len() < 2000 {
                        pending.push(entry.path());
                    }
                }
            } else if meta.len() <= MAX_FILE as u64 {
                bytes += meta.len() as usize;
                if let Ok(text) = self.read(&path) {
                    for (i, line) in text.lines().enumerate() {
                        if line.contains(query) {
                            output.push_str(&format!(
                                "{relative}:{}: {}\n",
                                i + 1,
                                bounded(line.into(), 500)
                            ));
                            if output.len() >= MAX_RESULT {
                                break;
                            }
                        }
                    }
                }
            }
        }
        output.push_str("\nKorlátos keresés: legfeljebb 2000 bejegyzés / 8 MiB; az üres találat nem bizonyítja a teljes projekt átvizsgálását.");
        Ok(output)
    }

    pub fn apply(&self, proposal: &EditProposal) -> Result<String> {
        let path = self.resolve(&proposal.path, true)?;
        let current = if path.exists() {
            Some(self.read(&path)?)
        } else {
            None
        };
        ensure!(
            current == proposal.before,
            "A fájl az előnézet óta változott. Kérj új módosítási tervet."
        );
        ensure!(proposal.after.len() <= MAX_FILE, "A módosítás túl nagy.");
        let parent = path.parent().context("Hiányzó szülőmappa")?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        temp.write_all(proposal.after.as_bytes())?;
        if path.exists() {
            temp.as_file()
                .set_permissions(fs::metadata(&path)?.permissions())?;
        }
        temp.as_file().sync_all()?;
        temp.persist(&path).map_err(|error| error.error)?;
        Ok(format!("Mentve: {}", proposal.path))
    }
}

fn blocked(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name);
    if matches!(stem, "con" | "prn" | "aux" | "nul")
        || stem
            .strip_prefix("com")
            .or_else(|| stem.strip_prefix("lpt"))
            .is_some_and(|suffix| suffix.len() == 1 && suffix.as_bytes()[0].is_ascii_digit())
    {
        return true;
    }
    matches!(
        name,
        ".git"
            | ".vibe"
            | ".ssh"
            | ".aws"
            | ".azure"
            | ".codex"
            | ".gemini"
            | "node_modules"
            | "target"
            | ".tools"
            | "speech"
            | "dist"
            | "credentials.json"
            | "auth.json"
            | "id_rsa"
            | "id_ed25519"
    ) || name == ".env"
        || name.starts_with(".env.") && name != ".env.example"
        || name.ends_with(".pem")
        || name.ends_with(".pfx")
        || name.ends_with(".key")
}

pub fn bounded(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut end = max;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
        text.push_str("\n[… csonkolva]");
    }
    text
}

pub fn definitions() -> Value {
    let tool = |name: &str, description: &str, properties: Value, required: Vec<&str>| {
        json!({
            "type":"function", "function": {"name":name,"description":description,
                "parameters":{"type":"object","properties":properties,"required":required,"additionalProperties":false}}
        })
    };
    json!([
        tool(
            "list_directory",
            "List a project directory; paths are workspace-relative.",
            json!({"path":{"type":"string"}}),
            vec!["path"]
        ),
        tool(
            "read_file",
            "Read a UTF-8 project file with line numbers (max 256 KiB).",
            json!({"path":{"type":"string"},"start_line":{"type":"integer"},"line_count":{"type":"integer"}}),
            vec!["path"]
        ),
        tool(
            "search_files",
            "Bounded literal text search inside project files; not regex.",
            json!({"path":{"type":"string"},"query":{"type":"string"}}),
            vec!["path", "query"]
        ),
        tool(
            "write_file",
            "Propose creating/replacing a UTF-8 file. User approval required; existing parent directory required. Read the original first. No deletion or shell execution.",
            json!({"path":{"type":"string"},"content":{"type":"string"}}),
            vec!["path", "content"]
        )
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser_lists_actual_directories_first_and_excludes_protected_entries() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::create_dir(dir.path().join("node_modules")).unwrap();
        fs::write(dir.path().join(".env"), "private").unwrap();
        fs::write(dir.path().join("a file.rs"), "file").unwrap();
        fs::write(dir.path().join("src/main.rs"), "code").unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        let root = workspace.list_directory(".").unwrap();
        assert_eq!(root.entries.len(), 2);
        assert!(root.entries[0].directory);
        assert_eq!(root.entries[1].name, "a file.rs");
        assert_eq!(
            workspace.list_directory("src").unwrap().entries[0].path,
            "src/main.rs"
        );
        assert_eq!(workspace.read_text("a file.rs").unwrap(), "file");
        assert!(workspace.list_directory("../").is_err());
        assert_eq!(
            workspace.change_path("gone/deep.rs").unwrap(),
            workspace.root.join("gone/deep.rs")
        );
        assert!(workspace.change_path("../escape.rs").is_err());
    }
    #[test]
    fn project_read_search_and_edit_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("main.rs"), "one\ntwo\n").unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        let Outcome::Text(read) = workspace
            .execute("read_file", &json!({"path":"main.rs","start_line":2}))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(read, "2 | two\n");
        let Outcome::Text(found) = workspace
            .execute("search_files", &json!({"query":"two"}))
            .unwrap()
        else {
            panic!()
        };
        assert!(found.contains("main.rs:2: two"));
        let Outcome::Edit(edit) = workspace
            .execute("write_file", &json!({"path":"main.rs","content":"changed"}))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(
            fs::read_to_string(dir.path().join("main.rs")).unwrap(),
            "one\ntwo\n"
        );
        fs::write(dir.path().join("main.rs"), "external edit").unwrap();
        assert!(workspace.apply(&edit).is_err());
        let Outcome::Edit(edit) = workspace
            .execute("write_file", &json!({"path":"new.rs","content":"new"}))
            .unwrap()
        else {
            panic!()
        };
        workspace.apply(&edit).unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join("new.rs")).unwrap(),
            "new"
        );
    }
    #[test]
    fn rejects_escape_secrets_binary_and_oversized_files() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        for path in [
            "../secret",
            "C:/Windows/file",
            ".env",
            ".git/config",
            "file:stream",
            ".env.local",
            "x.pem",
            "CON",
            "nul.txt",
            "COM1.log",
        ] {
            assert!(
                workspace
                    .execute("write_file", &json!({"path":path,"content":"x"}))
                    .is_err(),
                "{path}"
            );
        }
        fs::write(dir.path().join("binary"), [0, 1, 2]).unwrap();
        fs::write(dir.path().join("huge"), vec![b'a'; MAX_FILE + 1]).unwrap();
        for path in ["binary", "huge"] {
            assert!(
                workspace
                    .execute("read_file", &json!({"path":path}))
                    .is_err()
            );
        }
        let text = bounded("á".repeat(100), 31);
        assert!(text.contains("csonkolva"));
    }
    #[cfg(unix)]
    #[test]
    fn links_cannot_escape_or_alias_protected_paths() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("secret"), "secret").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("link")).unwrap();
        fs::write(dir.path().join(".env"), "secret").unwrap();
        std::os::unix::fs::symlink(dir.path().join(".env"), dir.path().join("innocent")).unwrap();
        let workspace = Workspace::new(dir.path()).unwrap();
        assert!(
            workspace
                .execute("read_file", &json!({"path":"link/secret"}))
                .is_err()
        );
        assert!(
            workspace
                .execute("read_file", &json!({"path":"innocent"}))
                .is_err()
        );
    }
}
