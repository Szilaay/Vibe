//! Versioned local transcripts. Credentials and provider configuration never enter this schema.
use std::{
    collections::HashSet,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::app::{
    App, Effort, LayoutMode, Message, Panel, PanelStatus, ProviderKind, RecentProject, Role, Usage,
};

const VERSION: u32 = 1;
const MAX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_WORKSPACE_PREFERENCES_BYTES: u64 = 1024 * 1024;

/// Sidebar preferences are separate from transcripts: removing an entry never deletes a session.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspacePreferences {
    version: u32,
    pinned: Vec<PathBuf>,
    hidden: Vec<PathBuf>,
    /// Remembers provider setup, never credentials or a claim of valid authentication.
    #[serde(default)]
    pub antigravity_enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_project: Option<PathBuf>,
}

impl Default for WorkspacePreferences {
    fn default() -> Self {
        Self {
            version: VERSION,
            pinned: Vec::new(),
            hidden: Vec::new(),
            antigravity_enabled: false,
            last_project: None,
        }
    }
}

impl WorkspacePreferences {
    /// Missing/deleted/hidden projects lead to the chooser, never directory creation.
    pub fn startup_project(&self) -> Option<PathBuf> {
        self.last_project
            .as_ref()
            .filter(|path| path.is_dir() && !self.is_hidden(path))
            .cloned()
    }

    pub fn is_pinned(&self, project: &Path) -> bool {
        self.pinned.iter().any(|path| path == project)
    }

    pub fn is_hidden(&self, project: &Path) -> bool {
        self.hidden.iter().any(|path| path == project)
    }

    pub fn set_pinned(&mut self, project: &Path, pinned: bool) {
        self.pinned.retain(|path| path != project);
        if pinned {
            self.pinned.push(project.into());
        }
    }

    pub fn remove(&mut self, project: &Path) {
        if self.last_project.as_deref() == Some(project) {
            self.last_project = None;
        }
        self.set_pinned(project, false);
        if !self.is_hidden(project) {
            self.hidden.push(project.into());
        }
    }

    pub fn reopen(&mut self, project: &Path) {
        self.hidden.retain(|path| path != project);
    }
}

pub fn workspace_preferences_path() -> Result<PathBuf> {
    let directories = ProjectDirs::from("dev", "Vibe", "Vibe")
        .context("Local application data directory is unavailable")?;
    Ok(directories.data_local_dir().join("workspaces.json"))
}

pub fn load_workspace_preferences(path: &Path) -> Result<WorkspacePreferences> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(WorkspacePreferences::default());
        }
        Err(error) => return Err(error).context("Cannot open workspace preferences"),
    };
    ensure!(
        file.metadata()?.len() <= MAX_WORKSPACE_PREFERENCES_BYTES,
        "Workspace preferences exceed 1 MiB limit"
    );
    let mut bytes = Vec::new();
    file.take(MAX_WORKSPACE_PREFERENCES_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_WORKSPACE_PREFERENCES_BYTES,
        "Workspace preferences exceed 1 MiB limit"
    );
    let preferences: WorkspacePreferences =
        serde_json::from_slice(&bytes).context("Invalid workspace preferences JSON")?;
    ensure!(
        preferences.version == VERSION,
        "Unsupported workspace preferences version"
    );
    ensure!(
        preferences
            .pinned
            .iter()
            .chain(&preferences.hidden)
            .chain(preferences.last_project.iter())
            .all(|path| path.is_absolute()),
        "Workspace preference paths must be absolute"
    );
    Ok(preferences)
}

pub fn save_workspace_preferences(path: &Path, preferences: &WorkspacePreferences) -> Result<()> {
    // Preserve corrupt or newer-version data exactly, as with transcript persistence.
    load_workspace_preferences(path)?;
    ensure!(
        preferences.version == VERSION
            && preferences
                .pinned
                .iter()
                .chain(&preferences.hidden)
                .chain(preferences.last_project.iter())
                .all(|path| path.is_absolute()),
        "Invalid workspace preferences"
    );
    let bytes = serde_json::to_vec_pretty(preferences)?;
    ensure!(
        bytes.len() as u64 <= MAX_WORKSPACE_PREFERENCES_BYTES,
        "Workspace preferences exceed 1 MiB limit"
    );
    atomic_write(path, &bytes)
}

#[derive(Debug)]
pub struct Session {
    pub workspace_groups: Vec<String>,
    pub active_group: Option<String>,
    pub panels: Vec<Panel>,
    pub active: usize,
    pub layout: LayoutMode,
    pub sidebar_visible: bool,
    pub next_id: u64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSession {
    #[serde(default)]
    workspace_groups: Vec<String>,
    #[serde(default)]
    active_group: Option<String>,
    version: u32,
    project: PathBuf,
    panels: Vec<StoredPanel>,
    active: usize,
    layout: LayoutMode,
    sidebar_visible: bool,
    next_id: u64,
}

// Explicit allowlist keeps future runtime fields out of persistent transcripts.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredPanel {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    attachments: Vec<String>,
    id: u64,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    group: Option<String>,
    model: String,
    #[serde(default)]
    provider: ProviderKind,
    #[serde(default)]
    effort: Effort,
    messages: Vec<Message>,
    input: String,
    usage: Option<Usage>,
    #[serde(default)]
    in_flight: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    terminal_key: Option<String>,
}

pub fn session_path(project: &Path) -> Result<PathBuf> {
    let project = project
        .canonicalize()
        .context("Cannot resolve project directory")?;
    ensure!(project.is_dir(), "Project must be a directory");
    let directories = ProjectDirs::from("dev", "Vibe", "Vibe")
        .context("Local application data directory is unavailable")?;
    let digest = Sha256::digest(project.as_os_str().as_encoded_bytes());
    Ok(directories
        .data_local_dir()
        .join("sessions")
        .join(format!("{digest:x}.json")))
}

pub fn recent_projects(limit: usize, preferences: &WorkspacePreferences) -> Vec<RecentProject> {
    let Some(directories) = ProjectDirs::from("dev", "Vibe", "Vibe") else {
        return Vec::new();
    };
    recent_projects_in(
        &directories.data_local_dir().join("sessions"),
        limit,
        preferences,
    )
}

fn recent_projects_in(
    directory: &Path,
    limit: usize,
    preferences: &WorkspacePreferences,
) -> Vec<RecentProject> {
    let mut found = Vec::new();
    let entries = fs::read_dir(directory).into_iter().flatten().flatten();
    for entry in entries.take(512) {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_BYTES {
            continue;
        }
        let Ok(bytes) = fs::read(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            continue;
        };
        let Some(project) = value.get("project").and_then(|value| value.as_str()) else {
            continue;
        };
        let project = PathBuf::from(project);
        if !project.is_dir() || preferences.is_hidden(&project) || is_system_workspace(&project) {
            continue;
        }
        let agents = value
            .get("panels")
            .and_then(|value| value.as_array())
            .map(|panels| panels.len())
            .unwrap_or(0);
        let name = project
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let modified = metadata
            .modified()
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let panels = value
            .get("panels")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|panel| {
                Some((
                    panel.get("id")?.as_u64()?,
                    panel.get("title")?.as_str()?.to_owned(),
                    panel
                        .get("provider")
                        .cloned()
                        .map(serde_json::from_value)
                        .transpose()
                        .ok()?
                        .unwrap_or_default(),
                ))
            })
            .collect::<Vec<_>>();
        found.push((modified, name, project, agents, panels));
    }
    // Pinned projects can exist before their first transcript has been saved.
    for project in &preferences.pinned {
        if project.is_dir()
            && !is_system_workspace(project)
            && !preferences.is_hidden(project)
            && !found.iter().any(|(_, _, path, _, _)| path == project)
        {
            found.push((
                std::time::SystemTime::UNIX_EPOCH,
                project
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into_owned(),
                project.clone(),
                0,
                Vec::new(),
            ));
        }
    }
    found.sort_by_key(|left| (!preferences.is_pinned(&left.2), std::cmp::Reverse(left.0)));
    let mut seen = HashSet::new();
    let mut found = found
        .into_iter()
        .filter(|(_, _, path, _, _)| seen.insert(path.clone()))
        .map(|(_, name, path, agents, panels)| RecentProject {
            pinned: preferences.is_pinned(&path),
            name,
            path,
            agents,
            panels,
        })
        .collect::<Vec<_>>();
    found.truncate(limit);
    found
}
pub fn is_system_workspace(project: &Path) -> bool {
    let Some(windows) = std::env::var_os("SystemRoot") else {
        return false;
    };
    let normalize = |path: &Path| {
        path.to_string_lossy()
            .replace('/', "\\")
            .trim_start_matches("\\\\?\\")
            .trim_end_matches('\\')
            .to_lowercase()
    };
    normalize(project) == normalize(&PathBuf::from(windows).join("System32"))
}

fn validate(stored: &mut StoredSession, project: &Path) -> Result<()> {
    ensure!(
        stored.version == VERSION,
        "Unsupported session version {}",
        stored.version
    );
    let project = project
        .canonicalize()
        .context("Cannot resolve project directory")?;
    ensure!(project.is_dir(), "Project must be a directory");
    ensure!(
        stored.project == project,
        "Session belongs to a different project"
    );
    ensure!(
        stored.panels.len() <= 8,
        "Session may contain up to 8 panels"
    );
    let mut ids = HashSet::new();
    let valid_name = |name: &str| {
        !name.trim().is_empty() && name.len() <= 256 && !name.chars().any(char::is_control)
    };
    let mut groups = HashSet::new();
    ensure!(
        stored.workspace_groups.len() <= 512,
        "Too many workspace groups"
    );
    for name in &stored.workspace_groups {
        ensure!(
            valid_name(name) && groups.insert(name),
            "Invalid or duplicate workspace group"
        );
    }
    ensure!(
        stored
            .active_group
            .as_ref()
            .is_none_or(|name| groups.contains(name)),
        "Unknown active workspace group"
    );
    let mut terminal_keys = HashSet::new();
    let mut maximum = 0;
    for panel in &stored.panels {
        ensure!(
            panel.description.len() <= 512 && !panel.description.chars().any(char::is_control),
            "Invalid chat description"
        );
        ensure!(
            panel
                .group
                .as_ref()
                .is_none_or(|name| groups.contains(name)),
            "Unknown chat group"
        );
        if let Some(key) = &panel.terminal_key {
            ensure!(
                terminal_keys.insert(key),
                "Terminal persistence keys must be unique"
            );
        }
        ensure!(
            panel
                .terminal_key
                .as_deref()
                .is_none_or(crate::terminal_persistence::valid_id),
            "Invalid terminal persistence key"
        );
        ensure!(
            panel.id != 0 && ids.insert(panel.id),
            "Panel IDs must be unique and nonzero"
        );
        ensure!(
            !panel.title.trim().is_empty()
                && panel.title.len() <= 256
                && !panel.title.chars().any(char::is_control),
            "Invalid panel title"
        );
        ensure!(
            !panel.model.is_empty()
                && panel.model.len() <= 200
                && !panel
                    .model
                    .chars()
                    .any(|ch| ch.is_control() || ch.is_whitespace()),
            "Invalid panel model"
        );
        ensure!(
            panel.input.len() <= 64 * 1024,
            "Panel draft exceeds 64 KiB limit"
        );
        ensure!(
            panel.attachments.len() <= 8
                && panel.attachments.iter().all(|path| !path.is_empty()
                    && path.len() <= 2048
                    && !path.contains(':')
                    && !path.chars().any(char::is_control)
                    && !path.starts_with(['/', '\\'])
                    && !path.split(['/', '\\']).any(|part| part == "..")),
            "Invalid project attachments"
        );
        ensure!(
            safe_text(&panel.input),
            "Invalid control character in panel draft"
        );
        ensure!(
            panel
                .messages
                .iter()
                .all(|message| safe_text(&message.content)),
            "Invalid control character in panel history"
        );
        maximum = maximum.max(panel.id);
    }
    let minimum = maximum.checked_add(1).context("Panel ID space exhausted")?;
    stored.next_id = stored.next_id.max(minimum);
    ensure!(stored.next_id < u64::MAX, "Panel ID space exhausted");
    stored.active = stored.active.min(stored.panels.len().saturating_sub(1));
    Ok(())
}

fn safe_text(text: &str) -> bool {
    !text
        .chars()
        .any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
}

pub fn load(path: &Path, project: &Path) -> Result<Option<Session>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("Cannot open local session"),
    };
    ensure!(
        file.metadata()?.len() <= MAX_BYTES,
        "Session exceeds 16 MiB limit"
    );
    let mut bytes = Vec::new();
    file.take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Cannot read local session")?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Session exceeds 16 MiB limit"
    );
    let mut stored: StoredSession =
        serde_json::from_slice(&bytes).context("Invalid local session JSON")?;
    validate(&mut stored, project)?;
    let panels = stored
        .panels
        .into_iter()
        .map(|mut panel| {
            if panel.in_flight {
                panel.messages.push(Message {
                    role: Role::System,
                    content: "Előző válasz megszakadt; részleges szöveg.".into(),
                });
            }
            Panel {
                attachments: panel.attachments,
                id: panel.id,
                title: panel.title,
                description: panel.description,
                group: panel.group,
                model: panel.model,
                provider: panel.provider,
                effort: panel.effort,
                messages: panel.messages,
                cursor: panel.input.len(),
                input: panel.input,
                scroll: 0,
                status: if panel.in_flight {
                    PanelStatus::Cancelled
                } else {
                    PanelStatus::Ready
                },
                usage: panel.usage,
                terminal_key: panel.terminal_key,
                terminal_saved: false,
            }
        })
        .collect();
    Ok(Some(Session {
        workspace_groups: stored.workspace_groups,
        active_group: stored.active_group,
        panels,
        active: stored.active,
        layout: stored.layout,
        sidebar_visible: stored.sidebar_visible,
        next_id: stored.next_id,
    }))
}

pub fn save(path: &Path, app: &App) -> Result<()> {
    // Never replace unreadable, corrupt, or future-version data with a fresh session.
    load(path, &app.project_path)
        .context("Existing session preserved; refusing to overwrite invalid data")?;
    let mut stored = StoredSession {
        workspace_groups: app.workspace_groups.clone(),
        active_group: app.active_group.clone(),
        version: VERSION,
        project: app
            .project_path
            .canonicalize()
            .context("Cannot resolve project directory")?,
        panels: app
            .panels
            .iter()
            .map(|panel| StoredPanel {
                attachments: panel.attachments.clone(),
                id: panel.id,
                title: panel.title.clone(),
                description: panel.description.clone(),
                group: panel.group.clone(),
                model: panel.model.clone(),
                provider: panel.provider,
                effort: panel.effort,
                messages: panel.messages.clone(),
                input: panel.input.clone(),
                usage: panel.usage,
                in_flight: panel.status == PanelStatus::Running,
                terminal_key: panel.terminal_key.clone(),
            })
            .collect(),
        active: app.active,
        layout: app.layout,
        sidebar_visible: app.sidebar_visible,
        next_id: app.next_id,
    };
    validate(&mut stored, &app.project_path)?;
    let bytes = serde_json::to_vec_pretty(&stored).context("Cannot encode local session")?;
    ensure!(
        bytes.len() as u64 <= MAX_BYTES,
        "Session exceeds 16 MiB limit"
    );
    atomic_write(path, &bytes)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).context("Cannot create session directory")?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).context("Cannot create session temporary file")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temporary
            .as_file()
            .set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    temporary
        .write_all(bytes)
        .context("Cannot write local session")?;
    temporary
        .as_file()
        .sync_all()
        .context("Cannot flush local session")?;
    // tempfile uses rename on Unix and MoveFileExW(REPLACE_EXISTING) on Windows.
    // The previous destination is never deleted before replacement.
    if let Err(error) = temporary.persist(path) {
        bail!("Cannot replace local session: {}", error.error);
    }
    #[cfg(unix)]
    File::open(parent)?
        .sync_all()
        .context("Cannot flush session directory")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn system32_is_never_listed_as_an_automatic_workspace() {
        if let Some(windows) = std::env::var_os("SystemRoot") {
            let path = PathBuf::from(windows).join("System32");
            assert!(is_system_workspace(&path));
            assert!(!is_system_workspace(&path.join("actual-project")));
        }
    }

    #[test]
    fn workspace_pins_and_removal_survive_reload_without_deleting_data() {
        let directory = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let project_path = project.path().canonicalize().unwrap();
        let other_path = other.path().canonicalize().unwrap();
        let session = directory.path().join("project.json");
        let other_session = directory.path().join("other.json");
        save(&session, &app(&project_path)).unwrap();
        save(&other_session, &app(&other_path)).unwrap();
        let original_session = fs::read(&session).unwrap();
        let project_file = project.path().join("important.txt");
        fs::write(&project_file, "project content").unwrap();
        let preferences_path = directory.path().join("preferences.data");
        let mut preferences = WorkspacePreferences::default();
        preferences.set_pinned(&project_path, true);
        save_workspace_preferences(&preferences_path, &preferences).unwrap();
        let restored = load_workspace_preferences(&preferences_path).unwrap();
        let recent = recent_projects_in(directory.path(), 1, &restored);
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].path, project_path);
        assert!(recent[0].pinned);

        preferences.remove(&project_path);
        preferences.remove(&project_path);
        save_workspace_preferences(&preferences_path, &preferences).unwrap();
        let mut restored = load_workspace_preferences(&preferences_path).unwrap();
        assert!(restored.is_hidden(&project_path));
        assert!(!restored.is_pinned(&project_path));
        assert_eq!(restored.hidden.len(), 1);
        assert!(
            recent_projects_in(directory.path(), 12, &restored)
                .iter()
                .all(|entry| entry.path != project_path)
        );
        assert_eq!(fs::read(&session).unwrap(), original_session);
        assert_eq!(fs::read_to_string(project_file).unwrap(), "project content");
        assert!(load(&session, &project_path).unwrap().is_some());

        restored.reopen(&project_path);
        save_workspace_preferences(&preferences_path, &restored).unwrap();
        let restored = load_workspace_preferences(&preferences_path).unwrap();
        assert!(
            recent_projects_in(directory.path(), 12, &restored)
                .iter()
                .any(|entry| entry.path == project_path)
        );
    }

    #[test]
    fn pinned_workspace_appears_without_a_saved_session_and_can_be_unpinned() {
        let directory = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let project_path = project.path().canonicalize().unwrap();
        let mut preferences = WorkspacePreferences::default();
        preferences.set_pinned(&project_path, true);
        let recent = recent_projects_in(&directory.path().join("missing"), 12, &preferences);
        assert_eq!(recent[0].path, project_path);
        assert!(recent[0].pinned);
        preferences.set_pinned(&project_path, false);
        assert!(!preferences.is_pinned(&project_path));
    }

    #[test]
    fn invalid_workspace_preferences_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("preferences.json");
        for bytes in [
            b"{truncated".as_slice(),
            br#"{"version":99,"pinned":[],"hidden":[]}"#,
            br#"{"version":1,"pinned":["relative"],"hidden":[]}"#,
        ] {
            fs::write(&path, bytes).unwrap();
            assert!(load_workspace_preferences(&path).is_err());
            assert!(save_workspace_preferences(&path, &WorkspacePreferences::default()).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
    }

    fn app(project: &Path) -> App {
        App {
            project_name: "Test".into(),
            project_path: project.into(),
            panels: (1..=2)
                .map(|id| Panel {
                    attachments: Vec::new(),
                    id,
                    title: format!("Panel {id}"),
                    description: String::new(),
                    group: None,
                    model: format!("provider/model-{id}"),
                    provider: ProviderKind::OpenRouter,
                    effort: Effort::default(),
                    messages: vec![Message {
                        role: Role::User,
                        content: format!("History {id}"),
                    }],
                    input: format!("Draft {id} árvíz 🦀"),
                    cursor: 0,
                    scroll: 12,
                    status: PanelStatus::Ready,
                    usage: Some(Usage {
                        prompt_tokens: id,
                        completion_tokens: id * 2,
                    }),
                    terminal_key: None,
                    terminal_saved: false,
                })
                .collect(),
            active: 1,
            layout: LayoutMode::Grid,
            sidebar_visible: false,
            next_id: 3,
            zoomed: false,
            show_help: false,
            model_editor: None,
            notice: "runtime-only-notice".into(),
            demo: true,
            ..App::new(project.into(), true)
        }
    }

    #[test]
    fn groups_and_chat_descriptions_survive_reload_and_legacy_sessions_still_load() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let mut app = app(directory.path());
        app.workspace_groups = vec!["Frontend".into(), "Backend".into()];
        app.active_group = Some("Frontend".into());
        app.panels[0].group = app.active_group.clone();
        app.panels[0].description = "Bejelentkezési felület".into();
        save(&path, &app).unwrap();
        let restored = load(&path, directory.path()).unwrap().unwrap();
        assert_eq!(restored.workspace_groups, app.workspace_groups);
        assert_eq!(restored.active_group, app.active_group);
        assert_eq!(restored.panels[0].group, app.panels[0].group);
        assert_eq!(restored.panels[0].description, app.panels[0].description);
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        legacy.as_object_mut().unwrap().remove("workspace_groups");
        legacy.as_object_mut().unwrap().remove("active_group");
        for panel in legacy["panels"].as_array_mut().unwrap() {
            panel.as_object_mut().unwrap().remove("description");
            panel.as_object_mut().unwrap().remove("group");
        }
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let restored = load(&path, directory.path()).unwrap().unwrap();
        assert!(restored.workspace_groups.is_empty());
        assert!(restored.active_group.is_none());
        assert!(restored.panels[0].description.is_empty());
        assert!(restored.panels[0].group.is_none());
    }

    #[test]
    fn roundtrip_preserves_independent_panels_and_replaces_existing_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let mut app = app(directory.path());
        app.panels[0].attachments = vec!["src/a file.rs".into()];
        assert!(load(&path, directory.path()).unwrap().is_none());
        save(&path, &app).unwrap();
        app.panels[1].input.push_str(" modified");
        save(&path, &app).unwrap();
        let restored = load(&path, directory.path()).unwrap().unwrap();
        assert_eq!(restored.active, 1);
        assert_eq!(restored.layout, LayoutMode::Grid);
        assert!(!restored.sidebar_visible);
        assert_eq!(restored.next_id, 3);
        for (actual, expected) in restored.panels.iter().zip(&app.panels) {
            assert_eq!(actual.id, expected.id);
            assert_eq!(actual.model, expected.model);
            assert_eq!(actual.messages[0].content, expected.messages[0].content);
            assert_eq!(actual.input, expected.input);
            assert_eq!(actual.attachments, expected.attachments);
            assert_eq!(actual.cursor, actual.input.len());
            assert!(actual.input.is_char_boundary(actual.cursor));
            assert_eq!(actual.status, PanelStatus::Ready);
            assert_eq!(actual.scroll, 0);
            assert_eq!(actual.usage.unwrap().prompt_tokens, expected.id);
        }
        let value: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(value.as_object().unwrap().len(), 9);
        assert_eq!(value["panels"][0].as_object().unwrap().len(), 12);
        assert_eq!(value["panels"][1].as_object().unwrap().len(), 11);
        for field in [
            "api_key",
            "credentials",
            "status",
            "cursor",
            "notice",
            "demo",
            "project_name",
        ] {
            assert!(value.get(field).is_none());
            assert!(value["panels"][0].get(field).is_none());
        }
    }

    #[test]
    fn crash_recovery_keeps_partial_answer_and_marks_interruption_once() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let mut app = app(directory.path());
        app.panels[0].status = PanelStatus::Running;
        app.panels[0].messages.push(Message {
            role: Role::Assistant,
            content: "Részleges válasz\n\ttovábbi sor".into(),
        });
        save(&path, &app).unwrap();
        let stored: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(stored["panels"][0]["in_flight"], true);
        assert_eq!(stored["panels"][1]["in_flight"], false);
        let recovered = load(&path, directory.path()).unwrap().unwrap();
        assert_eq!(recovered.panels[0].status, PanelStatus::Cancelled);
        assert_eq!(recovered.panels[1].status, PanelStatus::Ready);
        assert_eq!(
            recovered.panels[0].messages[1].content,
            "Részleges válasz\n\ttovábbi sor"
        );
        let interruption = recovered.panels[0].messages.last().unwrap();
        assert_eq!(interruption.role, Role::System);
        assert_eq!(
            interruption.content,
            "Előző válasz megszakadt; részleges szöveg."
        );
        app.panels = recovered.panels;
        save(&path, &app).unwrap();
        let reloaded = load(&path, directory.path()).unwrap().unwrap();
        assert_eq!(reloaded.panels[0].status, PanelStatus::Ready);
        assert_eq!(reloaded.panels[0].messages.len(), 3);
        assert_eq!(reloaded.panels[1].messages.len(), 1);
        // Existing pre-release v1 sessions do not require the new optional flag.
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for panel in legacy["panels"].as_array_mut().unwrap() {
            panel.as_object_mut().unwrap().remove("in_flight");
        }
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let legacy = load(&path, directory.path()).unwrap().unwrap();
        assert_eq!(legacy.panels[0].status, PanelStatus::Ready);
        assert_eq!(legacy.panels[0].messages.len(), 3);
    }

    #[test]
    fn startup_restores_selected_workspace_without_recreating_deleted_projects() {
        let data = tempfile::tempdir().unwrap();
        let project = data.path().join("selected");
        fs::create_dir(&project).unwrap();
        let path = data.path().join("workspaces.json");
        let mut preferences = WorkspacePreferences::default();
        assert!(preferences.startup_project().is_none());
        preferences.last_project = Some(project.clone());
        save_workspace_preferences(&path, &preferences).unwrap();
        let mut restored = load_workspace_preferences(&path).unwrap();
        assert_eq!(restored.startup_project(), Some(project.clone()));
        fs::remove_dir(&project).unwrap();
        assert!(restored.startup_project().is_none());
        assert!(!project.exists());
        fs::create_dir(&project).unwrap();
        restored.remove(&project);
        save_workspace_preferences(&path, &restored).unwrap();
        assert!(
            load_workspace_preferences(&path)
                .unwrap()
                .startup_project()
                .is_none()
        );
        assert!(
            project.is_dir(),
            "Removing a workspace never deletes its folder"
        );
    }

    #[test]
    fn google_setup_survives_restart_and_legacy_preferences_remain_valid() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workspaces.json");
        fs::write(&path, br#"{"version":1,"pinned":[],"hidden":[]}"#).unwrap();
        let mut preferences = load_workspace_preferences(&path).unwrap();
        assert!(!preferences.antigravity_enabled);
        preferences.antigravity_enabled = true;
        save_workspace_preferences(&path, &preferences).unwrap();
        assert!(
            load_workspace_preferences(&path)
                .unwrap()
                .antigravity_enabled
        );
        let stored: serde_json::Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
        assert_eq!(stored.as_object().unwrap().len(), 4);
        assert_eq!(stored["antigravity_enabled"], true);
    }

    #[test]
    fn rejects_invalid_data_and_preserves_original() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let app = app(directory.path());
        save(&path, &app).unwrap();
        let original: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        let mutations: [fn(&mut serde_json::Value); 17] = [
            |v| v["version"] = 99.into(),
            |v| v["panels"][1]["id"] = 1.into(),
            |v| v["panels"][0]["id"] = 0.into(),
            |v| v["panels"][0]["id"] = u64::MAX.into(),
            |v| v["panels"] = serde_json::Value::Array(vec![v["panels"][0].clone(); 9]),
            |v| v["panels"][0]["model"] = " ".into(),
            |v| v["panels"][0]["model"] = "x".repeat(201).into(),
            |v| v["panels"][0]["model"] = "provider/model id".into(),
            |v| v["panels"][0]["model"] = "provider/\u{1b}[2J".into(),
            |v| v["panels"][0]["title"] = "".into(),
            |v| v["panels"][0]["title"] = "x".repeat(257).into(),
            |v| v["panels"][0]["title"] = "title\nline".into(),
            |v| v["panels"][0]["input"] = "x".repeat(64 * 1024 + 1).into(),
            |v| v["panels"][0]["input"] = "\u{1b}[2J".into(),
            |v| v["panels"][0]["messages"][0]["content"] = "\u{1b}]52;c;secret\u{7}".into(),
            |v| v["next_id"] = u64::MAX.into(),
            |v| v["api_key"] = "not-allowed".into(),
        ];
        for mutate in mutations {
            let mut invalid = original.clone();
            mutate(&mut invalid);
            let bytes = serde_json::to_vec(&invalid).unwrap();
            fs::write(&path, &bytes).unwrap();
            assert!(load(&path, directory.path()).is_err());
            assert!(save(&path, &app).is_err());
            assert_eq!(fs::read(&path).unwrap(), bytes);
        }
        fs::write(&path, b"{truncated").unwrap();
        assert!(save(&path, &app).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"{truncated");
    }

    #[test]
    fn project_identity_bounds_and_id_repair() {
        let directory = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let mut app = app(directory.path());
        app.active = usize::MAX;
        app.next_id = 0;
        save(&path, &app).unwrap();
        let loaded = load(&path, directory.path()).unwrap().unwrap();
        assert_eq!(loaded.active, 1);
        assert_eq!(loaded.next_id, 3);
        assert!(load(&path, other.path()).is_err());
        assert_eq!(
            session_path(directory.path()).unwrap(),
            session_path(&directory.path().join(".")).unwrap()
        );
        assert_ne!(
            session_path(directory.path()).unwrap(),
            session_path(other.path()).unwrap()
        );
        File::create(&path).unwrap().set_len(MAX_BYTES + 1).unwrap();
        assert!(load(&path, directory.path()).is_err());
    }

    #[test]
    fn oversized_save_preserves_existing_session() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        let mut app = app(directory.path());
        save(&path, &app).unwrap();
        let original = fs::read(&path).unwrap();
        app.panels[0].messages[0].content = "x".repeat(MAX_BYTES as usize);
        assert!(save(&path, &app).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn provider_roundtrip_and_legacy_default_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("providers.json");
        let mut app = app(directory.path());
        app.panels[0].provider = ProviderKind::Chatgpt;
        app.panels[1].provider = ProviderKind::Antigravity;
        app.panels[1].model = "antigravity/default".into();
        save(&path, &app).unwrap();
        let restored = load(&path, directory.path()).unwrap().unwrap();
        assert_eq!(restored.panels[0].provider, ProviderKind::Chatgpt);
        assert_eq!(restored.panels[1].provider, ProviderKind::Antigravity);
        assert_eq!(restored.panels[1].model, "antigravity/default");
        assert_eq!(restored.panels[0].effort, Effort::default());
        let mut legacy: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        for panel in legacy["panels"].as_array_mut().unwrap() {
            panel.as_object_mut().unwrap().remove("provider");
        }
        fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
        let restored = load(&path, directory.path()).unwrap().unwrap();
        assert!(
            restored
                .panels
                .iter()
                .all(|panel| panel.provider == ProviderKind::OpenRouter)
        );
    }

    #[cfg(unix)]
    #[test]
    fn session_permissions_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("session.json");
        save(&path, &app(directory.path())).unwrap();
        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
