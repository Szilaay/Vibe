use std::{collections::HashMap, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PanelStatus {
    #[default]
    Ready,
    Running,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    #[default]
    #[value(name = "openrouter")]
    OpenRouter,
    Chatgpt,
    Antigravity,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effort {
    #[default]
    Fast,
    Balanced,
    Deep,
}

impl Effort {
    pub const ALL: [Self; 3] = [Self::Fast, Self::Balanced, Self::Deep];

    pub fn label(self) -> &'static str {
        match self {
            Self::Fast => "Fast",
            Self::Balanced => "Balanced",
            Self::Deep => "Deep",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Fast => "Quick answers with minimal reasoning",
            Self::Balanced => "Good balance of speed and depth",
            Self::Deep => "More reasoning for difficult tasks",
        }
    }

    pub fn reasoning(self) -> &'static str {
        match self {
            Self::Fast => "low",
            Self::Balanced => "medium",
            Self::Deep => "high",
        }
    }
}

impl ProviderKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::OpenRouter => "OpenRouter",
            Self::Chatgpt => "ChatGPT",
            Self::Antigravity => "Antigravity",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Chatgpt | Self::Antigravity)
    }
}

#[derive(Debug, Clone)]
pub struct RecentProject {
    pub name: String,
    pub path: PathBuf,
    pub agents: usize,
    pub pinned: bool,
    pub panels: Vec<(u64, String, ProviderKind)>,
}

#[derive(Debug, Clone)]
pub struct ModelChoice {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Panel {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<String>,
    pub id: u64,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub group: Option<String>,
    pub model: String,
    #[serde(default)]
    pub provider: ProviderKind,
    #[serde(default)]
    pub effort: Effort,
    pub messages: Vec<Message>,
    pub input: String,
    #[serde(skip)]
    pub cursor: usize,
    #[serde(skip)]
    pub scroll: u16,
    #[serde(skip)]
    pub status: PanelStatus,
    pub usage: Option<Usage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_key: Option<String>,
    #[serde(skip)]
    pub terminal_saved: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LayoutMode {
    #[default]
    Focus,
    Columns,
    Grid,
}

impl LayoutMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Focus => "Split",
            Self::Columns => "Columns",
            Self::Grid => "Grid",
        }
    }
    pub fn next(self) -> Self {
        match self {
            Self::Focus => Self::Columns,
            Self::Columns => Self::Grid,
            Self::Grid => Self::Focus,
        }
    }
}

#[derive(Debug)]
pub struct App {
    pub files: crate::project_files::Browser,
    pub edit_reviews: HashMap<u64, crate::workspace_tools::EditProposal>,
    pub workspace_groups: Vec<String>,
    pub active_group: Option<String>,
    pub project_name: String,
    pub project_path: PathBuf,
    pub panels: Vec<Panel>,
    pub active: usize,
    pub layout: LayoutMode,
    pub sidebar_visible: bool,
    pub zoomed: bool,
    pub show_help: bool,
    pub model_editor: Option<String>,
    pub notice: String,
    pub demo: bool,
    pub next_id: u64,
    pub account_label: String,
    pub account_usage: crate::account_usage::AccountUsageState,
    pub antigravity_enabled: bool,
    pub antigravity_usage: crate::antigravity_usage::AntigravityUsageState,
    pub chatgpt_ready: bool,
    pub siwc_ready: bool,
    pub siwc_account_label: String,
    pub native_codex_account: Option<crate::codex_account::AccountInfo>,
    pub native_codex_busy: bool,
    pub native_codex_login_url: Option<String>,
    pub terminal_notices: HashMap<u64, String>,
    pub auth_busy: bool,
    pub login_url: Option<String>,
    pub available_models: Vec<ModelChoice>,
    pub model_selection: usize,
    pub recent_projects: Vec<RecentProject>,
}

impl App {
    /// Unbound welcome UI: no project, session, default panel or working-directory fallback.
    pub fn welcome() -> Self {
        let mut app = Self::new(PathBuf::new(), false);
        app.project_name = "VIBE".into();
        app.panels.clear();
        app.recent_projects.clear();
        app.workspace_groups.clear();
        app.active_group = None;
        app.next_id = 1;
        app.notice = "Válassz CLI-t; a projektmappát a következő lépésben adhatod meg.".into();
        app
    }
    pub fn new(project_path: PathBuf, demo: bool) -> Self {
        let project_name = project_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let mut app = Self {
            files: Default::default(),
            edit_reviews: HashMap::new(),
            workspace_groups: Vec::new(),
            active_group: None,
            project_name: project_name.clone(),
            project_path: project_path.clone(),
            panels: Vec::new(),
            active: 0,
            layout: LayoutMode::Focus,
            sidebar_visible: true,
            zoomed: false,
            show_help: false,
            model_editor: None,
            notice: if demo {
                "DEMO - local sample replies, no network requests".into()
            } else {
                "F1: help - Ctrl+N: new agent - Enter: run".into()
            },
            demo,
            next_id: 1,
            account_label: "Not signed in".into(),
            account_usage: crate::account_usage::AccountUsageState::default(),
            antigravity_enabled: false,
            antigravity_usage: crate::antigravity_usage::AntigravityUsageState::default(),
            chatgpt_ready: false,
            siwc_ready: false,
            siwc_account_label: "Not signed in".into(),
            native_codex_account: None,
            native_codex_busy: false,
            native_codex_login_url: None,
            terminal_notices: HashMap::new(),
            auth_busy: false,
            login_url: None,
            available_models: Vec::new(),
            model_selection: 0,
            recent_projects: vec![RecentProject {
                name: project_name.clone(),
                path: project_path.clone(),
                agents: 0,
                pinned: false,
                panels: Vec::new(),
            }],
        };
        for (title, model) in [
            ("Claude Code", "anthropic/claude-sonnet-5.5"),
            ("Codex", "openai/gpt-6.1-sol"),
            ("Gemini CLI", "google/gemini-3.8-flash"),
        ] {
            app.add_panel(title.into(), model.into());
        }
        app.active = 0;
        app.recent_projects[0].agents = app.panels.len();
        if demo {
            let examples = [
                (
                    "Describe the Rust harness architecture.",
                    "DEMO - illustrative reply.\n\nThe Rust core keeps panel state, provider connections, and local persistence separate.\n\nEach agent gets its own thread. OpenRouter requests can run in parallel while the UI stays responsive.\n\nNext step: file tools and an approval-based agent loop.",
                ),
                (
                    "How should agents share context?",
                    "DEMO - illustrative reply.\n\nEach thread stores independently:\n  - the model identifier,\n  - history and the current draft,\n  - response status and token usage.\n\nCtrl+Tab cycles agents. Enter runs the prompt.",
                ),
                (
                    "Review the multi-agent workspace.",
                    "DEMO - illustrative reply.\n\nINDEPENDENT THREADS\nNeighbouring agents never inject messages into this thread.\n\nPARALLEL RESPONSES\nEvery agent owns its response stream. Esc stops only the active one.\n\nLAYOUT\nUse the layout selector or drag the splitters.\n\nType a message to try the local demo stream. The demo never calls a model and never edits project files.",
                ),
            ];
            for (panel, (question, answer)) in app.panels.iter_mut().zip(examples) {
                panel.messages = vec![
                    Message {
                        role: Role::User,
                        content: question.into(),
                    },
                    Message {
                        role: Role::Assistant,
                        content: answer.into(),
                    },
                ];
            }
        }
        app
    }

    pub fn remember_project(&mut self, name: String, path: PathBuf, agents: usize) {
        let pinned = self
            .recent_projects
            .iter()
            .any(|project| project.path == path && project.pinned);
        self.recent_projects.retain(|project| project.path != path);
        self.recent_projects.insert(
            0,
            RecentProject {
                name,
                path,
                agents,
                pinned,
                panels: Vec::new(),
            },
        );
        self.recent_projects.sort_by_key(|project| !project.pinned);
        self.recent_projects.truncate(12);
    }

    pub fn active_panel(&self) -> &Panel {
        &self.panels[self.active]
    }
    pub fn active_panel_mut(&mut self) -> &mut Panel {
        &mut self.panels[self.active]
    }

    pub fn next_panel_name(&self) -> String {
        const NAMES: [&str; 16] = [
            "Emma",
            "Olivia",
            "Amelia",
            "Charlotte",
            "Sophia",
            "Grace",
            "Lily",
            "Emily",
            "Alice",
            "Lucy",
            "Chloe",
            "Sophie",
            "Rose",
            "Hannah",
            "Ella",
            "Ruby",
        ];
        NAMES
            .iter()
            .cycle()
            .skip((self.next_id.saturating_sub(1) % NAMES.len() as u64) as usize)
            .take(NAMES.len())
            .find(|name| {
                !self
                    .panels
                    .iter()
                    .any(|panel| panel.title.eq_ignore_ascii_case(name))
            })
            .expect("more names than the maximum number of panels")
            .to_string()
    }

    pub fn add_panel(&mut self, title: String, model: String) -> bool {
        if self.panels.len() >= 8 {
            self.notice = "Up to 8 agents can be open.".into();
            return false;
        }
        let id = self.next_id;
        let Some(next_id) = self.next_id.checked_add(1).filter(|id| *id < u64::MAX) else {
            self.notice = "Cannot create an agent: identifier space exhausted.".into();
            return false;
        };
        self.next_id = next_id;
        let intro = if self.demo {
            "DEMO AGENT - replies are local samples. No file or model requests happen."
        } else {
            "VIBE Agent · Projektfájlok olvasása és keresése · Írás előnézettel · /help"
        };
        self.panels.push(Panel {
            attachments: Vec::new(),
            id,
            title,
            description: String::new(),
            group: self.active_group.clone(),
            model,
            provider: ProviderKind::OpenRouter,
            effort: Effort::default(),
            messages: vec![Message {
                role: Role::System,
                content: intro.into(),
            }],
            input: String::new(),
            cursor: 0,
            scroll: 0,
            status: PanelStatus::Ready,
            usage: None,
            terminal_key: None,
            terminal_saved: false,
        });
        self.active = self.panels.len() - 1;
        self.zoomed = false;
        true
    }

    pub fn close_active(&mut self) -> Option<u64> {
        if self.panels.len() == 1 {
            self.notice = "At least one agent must stay open.".into();
            return None;
        }
        let id = self.panels.remove(self.active).id;
        self.active = self.active.min(self.panels.len() - 1);
        Some(id)
    }

    pub fn focus_next(&mut self, backwards: bool) {
        let count = self.panels.len();
        self.active = if backwards {
            (self.active + count - 1) % count
        } else {
            (self.active + 1) % count
        };
        self.active_group = self.panels[self.active].group.clone();
    }

    pub fn focus_id(&mut self, id: u64) {
        if let Some(index) = self.panels.iter().position(|panel| panel.id == id) {
            self.active = index;
            self.active_group = self.panels[index].group.clone();
        }
    }

    pub fn insert(&mut self, text: &str) {
        // Keep drafts bounded and prevent terminal control sequences in pasted input.
        let text: String = text
            .chars()
            .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
            .collect();
        let panel = self.active_panel_mut();
        if panel.input.len().saturating_add(text.len()) > 64 * 1024 {
            return;
        }
        panel.input.insert_str(panel.cursor, &text);
        panel.cursor += text.len();
    }

    pub fn backspace(&mut self) {
        let panel = self.active_panel_mut();
        if let Some((prev, _)) = panel.input[..panel.cursor].char_indices().next_back() {
            panel.input.drain(prev..panel.cursor);
            panel.cursor = prev;
        }
    }

    pub fn delete(&mut self) {
        let panel = self.active_panel_mut();
        if let Some(ch) = panel.input[panel.cursor..].chars().next() {
            panel
                .input
                .drain(panel.cursor..panel.cursor + ch.len_utf8());
        }
    }

    pub fn move_cursor(&mut self, right: bool) {
        let panel = self.active_panel_mut();
        if right {
            if let Some(ch) = panel.input[panel.cursor..].chars().next() {
                panel.cursor += ch.len_utf8();
            }
        } else if let Some((prev, _)) = panel.input[..panel.cursor].char_indices().next_back() {
            panel.cursor = prev;
        }
    }

    pub fn set_model(&mut self, model: String) -> bool {
        let model = model.trim();
        if model.is_empty()
            || model.len() > 200
            || model.chars().any(char::is_whitespace)
            || model.chars().any(char::is_control)
        {
            self.notice = "Invalid model identifier. Example: openrouter/auto".into();
            return false;
        }
        if self.active_panel().status == PanelStatus::Running {
            self.notice = "Stop the active response before changing the model: Esc.".into();
            return false;
        }
        self.active_panel_mut().model = model.into();
        self.notice = "Agent model updated. The next request uses it.".into();
        true
    }

    pub fn set_provider(&mut self, provider: ProviderKind) -> bool {
        if self.active_panel().status == PanelStatus::Running {
            self.notice = "Stop the active response before changing the provider: Esc.".into();
            return false;
        }
        if self.active_panel().provider == provider {
            return true;
        }
        let model = match provider {
            ProviderKind::Chatgpt => self
                .available_models
                .first()
                .map(|model| model.id.clone())
                .unwrap_or_else(|| "gpt-6.1-sol".into()),
            ProviderKind::OpenRouter => "openrouter/auto".into(),
            ProviderKind::Antigravity => "antigravity/default".into(),
        };
        let panel = self.active_panel_mut();
        panel.provider = provider;
        panel.terminal_key = None;
        panel.terminal_saved = false;
        panel.model = model;
        if panel.title.starts_with("Claude ·")
            || panel.title.starts_with("Gemini ·")
            || panel.title.starts_with("OpenAI ·")
            || matches!(panel.title.as_str(), "Claude Code" | "Codex" | "Gemini CLI")
        {
            panel.title = format!(
                "{} · Chat {}",
                match provider {
                    ProviderKind::Chatgpt => "OpenAI",
                    ProviderKind::Antigravity => "Antigravity",
                    ProviderKind::OpenRouter => "AI",
                },
                panel.id
            );
        }
        self.notice = format!(
            "Agent provider: {}. The next request sends history here.",
            provider.label()
        );
        true
    }

    pub fn set_effort(&mut self, effort: Effort) -> bool {
        if self.active_panel().status == PanelStatus::Running {
            self.notice = "Stop the active response before changing effort: Esc.".into();
            return false;
        }
        self.active_panel_mut().effort = effort;
        self.notice = format!("Model effort: {}.", effort.label());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_and_model_are_independent_per_panel() {
        let mut app = App::new(PathBuf::from("demo"), true);
        app.insert("első");
        let first_model = app.active_panel().model.clone();
        app.focus_next(false);
        app.insert("második");
        app.set_model("openrouter/auto".into());
        assert_eq!(app.panels[0].input, "első");
        assert_eq!(app.panels[0].model, first_model);
        assert_eq!(app.panels[1].input, "második");
        assert_eq!(app.panels[1].model, "openrouter/auto");
    }

    #[test]
    fn unicode_editing_respects_byte_boundaries() {
        let mut app = App::new(PathBuf::from("demo"), true);
        app.insert("ár🦀víz");
        for _ in 0..4 {
            app.move_cursor(false);
        }
        app.delete();
        app.backspace();
        assert_eq!(app.active_panel().input, "ávíz");
        app.insert("ő");
        assert_eq!(app.active_panel().input, "áővíz");
    }

    #[test]
    fn effort_defaults_and_updates_per_active_panel() {
        let mut app = App::new(PathBuf::from("demo"), true);
        assert_eq!(app.panels[0].effort, Effort::Fast);
        assert!(app.set_effort(Effort::Deep));
        assert_eq!(app.panels[0].effort, Effort::Deep);
        app.focus_next(false);
        assert_eq!(app.panels[1].effort, Effort::Fast);
    }

    #[test]
    fn panel_limit_and_last_panel_are_enforced() {
        let mut app = App::new(PathBuf::from("demo"), true);
        for _ in 0..20 {
            app.add_panel("Chat".into(), "openrouter/auto".into());
        }
        assert_eq!(app.panels.len(), 8);
        for _ in 0..20 {
            app.close_active();
        }
        assert_eq!(app.panels.len(), 1);
        assert_eq!(app.active, 0);
    }
}
