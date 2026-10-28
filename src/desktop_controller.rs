use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    fs::{File, OpenOptions},
    hash::{Hash, Hasher},
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use eframe::egui;
use fs2::FileExt;
use tokio::{runtime::Handle, sync::mpsc, task::JoinHandle};

use crate::{
    account_usage::{self, AccountUsage},
    antigravity,
    antigravity_usage::{self, AntigravityUsage},
    app::{App, Message, ModelChoice, PanelStatus, ProviderKind, Role},
    auth::{self, LoginEvent},
    chatgpt, codex_account,
    desktop_types::DesktopAction,
    provider::{self, ChatRequest, ProviderEvent},
    storage,
    terminal_backend::TerminalSession,
    terminal_persistence::TerminalStore,
};

enum Event {
    Files {
        root: PathBuf,
        request: crate::project_files::Request,
        generation: u64,
        result: std::result::Result<crate::project_files::Response, String>,
    },
    Usage(u64, std::result::Result<AccountUsage, String>),
    AntigravityUsage(u64, std::result::Result<AntigravityUsage, String>),
    Provider(u64, ProviderEvent),
    Login(u64, LoginEvent),
    Account(u64, AccountResult),
    NativeLogin(u64, codex_account::LoginEvent),
    NativeAccount(
        u64,
        std::result::Result<Option<codex_account::AccountInfo>, String>,
    ),
    NativeLogout(u64, std::result::Result<(), String>),
    TerminalProgress(u64, u64, String),
    Codex(u64, u64, std::result::Result<TerminalSession, String>),
    TerminalSaveFailed(String),
    Shutdown,
}

enum AccountResult {
    Status(std::result::Result<Option<auth::AccountInfo>, String>),
    Models(std::result::Result<Vec<chatgpt::ModelInfo>, String>),
    Logout(std::result::Result<bool, String>),
}

struct Worker {
    generation: u64,
    task: JoinHandle<()>,
}
struct AccountJob {
    task: JoinHandle<()>,
    logout: bool,
}
struct CodexPanel {
    generation: u64,
    cancel: tokio::sync::watch::Sender<bool>,
    task: JoinHandle<()>,
}

pub struct DesktopController {
    pub app: App,
    pub terminals: HashMap<u64, TerminalSession>,
    runtime: Handle,
    repaint: egui::Context,
    sender: mpsc::UnboundedSender<Event>,
    receiver: mpsc::UnboundedReceiver<Event>,
    workers: HashMap<u64, Worker>,
    edit_replies: HashMap<u64, tokio::sync::oneshot::Sender<bool>>,
    file_jobs: HashMap<crate::project_files::Request, (u64, JoinHandle<()>)>,
    codex: HashMap<u64, CodexPanel>,
    retired: Vec<JoinHandle<()>>,
    generation: u64,
    account_generation: u64,
    native_generation: u64,
    usage_generation: u64,
    usage_job: Option<JoinHandle<()>>,
    usage_last_attempt: Option<Instant>,
    live_usage: crate::live_usage::LiveUsageRefresh,
    antigravity_usage_generation: u64,
    antigravity_usage_job: Option<JoinHandle<()>>,
    antigravity_usage_last_attempt: Option<Instant>,
    login: Option<JoinHandle<()>>,
    account_job: Option<AccountJob>,
    native_login: Option<JoinHandle<()>>,
    native_job: Option<AccountJob>,
    shutdown: Option<JoinHandle<()>>,
    save_path: Option<PathBuf>,
    project_lock: Option<File>,
    workspace_preferences: storage::WorkspacePreferences,
    workspace_preferences_path: Option<PathBuf>,
    no_save: bool,
    openrouter_key: String,
    last_save: Instant,
    saved_fingerprint: Option<u64>,
    closing: bool,
    ready_to_close: bool,
    terminal_save_error: Option<String>,
    plan_limited: bool,
    startup_terminals: Vec<u64>,
}

impl DesktopController {
    pub fn new(
        project: PathBuf,
        demo: bool,
        no_save: bool,
        repaint: egui::Context,
        runtime: Handle,
    ) -> Result<Self> {
        let openrouter_key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
        let (mut app, save_path, project_lock) =
            load_project(&project, demo, no_save, &openrouter_key)?;
        let terminals = restore_saved_terminals(&mut app, &repaint, demo || no_save)?;
        let (sender, receiver) = mpsc::unbounded_channel();
        let workspace_preferences_path = if demo || no_save {
            None
        } else {
            Some(storage::workspace_preferences_path()?)
        };
        let workspace_preferences = workspace_preferences_path
            .as_deref()
            .map(storage::load_workspace_preferences)
            .transpose()?
            .unwrap_or_default();
        let mut controller = Self {
            app,
            terminals,
            runtime,
            repaint,
            sender,
            receiver,
            workers: HashMap::new(),
            edit_replies: HashMap::new(),
            file_jobs: HashMap::new(),
            codex: HashMap::new(),
            retired: Vec::new(),
            generation: 0,
            account_generation: 0,
            native_generation: 0,
            usage_generation: 0,
            usage_job: None,
            usage_last_attempt: None,
            live_usage: Default::default(),
            antigravity_usage_generation: 0,
            antigravity_usage_job: None,
            antigravity_usage_last_attempt: None,
            login: None,
            account_job: None,
            native_login: None,
            native_job: None,
            shutdown: None,
            save_path,
            project_lock,
            workspace_preferences,
            workspace_preferences_path,
            no_save,
            openrouter_key,
            last_save: Instant::now(),
            saved_fingerprint: None,
            closing: false,
            ready_to_close: false,
            terminal_save_error: None,
            plan_limited: false,
            startup_terminals: Vec::new(),
        };
        if controller.workspace_preferences_path.is_some()
            && (controller.workspace_preferences.last_project.as_ref()
                != Some(&controller.app.project_path)
                || controller
                    .workspace_preferences
                    .is_hidden(&controller.app.project_path))
        {
            let mut preferences = controller.workspace_preferences.clone();
            preferences.last_project = Some(controller.app.project_path.clone());
            preferences.reopen(&controller.app.project_path);
            controller.update_workspace_preferences(preferences)?;
        }
        // Migrate existing Antigravity panels from versions without a provider preference.
        if !controller.workspace_preferences.antigravity_enabled
            && controller
                .app
                .panels
                .iter()
                .any(|panel| panel.provider == ProviderKind::Antigravity)
            && !controller.app.demo
        {
            let mut preferences = controller.workspace_preferences.clone();
            preferences.antigravity_enabled = true;
            controller.update_workspace_preferences(preferences)?;
        }
        controller.app.antigravity_enabled = controller.workspace_preferences.antigravity_enabled;
        controller.refresh_recent_projects();
        controller.queue_startup_terminals();
        if !controller.app.demo {
            controller.begin_status();
            controller.begin_native_status();
        }
        Ok(controller)
    }

    fn refresh_recent_projects(&mut self) {
        let mut projects = storage::recent_projects(512, &self.workspace_preferences);
        if !self.workspace_preferences.is_hidden(&self.app.project_path)
            && !projects
                .iter()
                .any(|project| project.path == self.app.project_path)
        {
            projects.insert(
                0,
                crate::app::RecentProject {
                    name: self.app.project_name.clone(),
                    path: self.app.project_path.clone(),
                    agents: self.app.panels.len(),
                    pinned: self.workspace_preferences.is_pinned(&self.app.project_path),
                    panels: Vec::new(),
                },
            );
        }
        projects.sort_by_key(|project| !project.pinned);
        // Always retain the visible active workspace, even with many pinned entries.
        if projects.len() > 12 {
            if let Some(index) = projects
                .iter()
                .position(|project| project.path == self.app.project_path)
                && index >= 12
            {
                let current = projects.remove(index);
                projects.truncate(11);
                projects.push(current);
                projects.sort_by_key(|project| !project.pinned);
            } else {
                projects.truncate(12);
            }
        }
        self.app.recent_projects = projects;
        if let Some(current) = self
            .app
            .recent_projects
            .iter_mut()
            .find(|project| project.path == self.app.project_path)
        {
            current.agents = self.app.panels.len();
        }
    }

    fn update_workspace_preferences(
        &mut self,
        preferences: storage::WorkspacePreferences,
    ) -> Result<()> {
        if let Some(path) = &self.workspace_preferences_path {
            storage::save_workspace_preferences(path, &preferences)
                .context("Cannot save workspace list preferences")?;
        }
        self.workspace_preferences = preferences;
        Ok(())
    }

    fn set_project_pinned(&mut self, project: PathBuf, pinned: bool) -> Result<()> {
        let project = project
            .canonicalize()
            .context("Cannot resolve workspace directory")?;
        let mut preferences = self.workspace_preferences.clone();
        preferences.set_pinned(&project, pinned);
        self.update_workspace_preferences(preferences)?;
        self.refresh_recent_projects();
        self.app.notice = if pinned {
            "Workspace kitűzve."
        } else {
            "Workspace kitűzése megszüntetve."
        }
        .into();
        Ok(())
    }

    fn remove_project(&mut self, project: PathBuf) -> Result<()> {
        let project = project.canonicalize().unwrap_or(project);
        ensure!(project.is_absolute(), "Workspace path must be absolute");
        if project == self.app.project_path {
            self.cancel_all(false);
            ensure!(self.save(false), "{}", self.app.notice);
        }
        let mut preferences = self.workspace_preferences.clone();
        preferences.remove(&project);
        self.update_workspace_preferences(preferences)?;
        self.refresh_recent_projects();
        self.app.notice =
            "Workspace eltávolítva a listából. Projektfájlok és beszélgetések megmaradtak.".into();
        Ok(())
    }
    pub fn poll(&mut self) {
        for _ in 0..4096 {
            let Ok(event) = self.receiver.try_recv() else {
                break;
            };
            match event {
                Event::TerminalSaveFailed(message) => {
                    self.terminal_save_error = Some(message.clone());
                    self.app.notice = format!("CLI mentése sikertelen: {message}");
                }
                Event::Shutdown => {
                    if let Some(message) = self.terminal_save_error.take() {
                        self.app.notice = format!("CLI mentése sikertelen: {message}");
                        self.closing = false;
                        continue;
                    }
                    if self.save(false) {
                        self.ready_to_close = true;
                    } else {
                        // Keep the window and project lock alive so failed persistence is visible.
                        self.closing = false;
                        self.app.auth_busy = false;
                        self.app.login_url = None;
                    }
                }
                Event::Codex(_, _, Ok(session)) if self.closing => self.retire_terminal(session),
                _ if self.closing => {}
                Event::Files {
                    root,
                    request,
                    generation,
                    result,
                } => {
                    if root == self.app.project_path
                        && self
                            .file_jobs
                            .get(&request)
                            .is_some_and(|(current, _)| *current == generation)
                    {
                        self.file_jobs.remove(&request);
                        self.app.files.complete(&request, result);
                    }
                }
                Event::Provider(generation, event) => self.provider_event(generation, event),
                Event::Usage(generation, result) => self.usage_event(generation, result),
                Event::AntigravityUsage(generation, result) => {
                    self.antigravity_usage_event(generation, result)
                }
                Event::Codex(id, generation, status) => self.codex_status(id, generation, status),
                Event::TerminalProgress(id, generation, notice)
                    if self
                        .codex
                        .get(&id)
                        .is_some_and(|job| job.generation == generation) =>
                {
                    self.app.terminal_notices.insert(id, notice);
                }
                Event::NativeLogin(generation, event) if generation == self.native_generation => {
                    self.native_login_event(event)
                }
                Event::NativeAccount(generation, result)
                    if generation == self.native_generation =>
                {
                    self.finish_native_job();
                    match result {
                        Ok(info) => self.set_native_account(info),
                        Err(message) => self.app.notice = message,
                    }
                }
                Event::NativeLogout(generation, result) if generation == self.native_generation => {
                    self.finish_native_job();
                    match result {
                        Ok(()) => self.app.notice = "Codex fiók leválasztva.".into(),
                        Err(message) => {
                            self.app.notice = message;
                            self.begin_native_status();
                        }
                    }
                }
                Event::Login(generation, event) if generation == self.account_generation => {
                    self.login_event(event)
                }
                Event::Account(generation, event) if generation == self.account_generation => {
                    self.account_event(event)
                }
                _ => {}
            }
        }
        self.retired.retain(|task| !task.is_finished());
        self.start_queued_terminals();
        self.update_live_usage();
        if !self.closing && self.last_save.elapsed() >= Duration::from_secs(2) {
            self.save(false);
        }
        if !self.receiver.is_empty() {
            self.repaint.request_repaint();
        }
        if !self.closing {
            self.repaint.request_repaint_after(Duration::from_secs(2));
        }
    }

    pub fn action(&mut self, action: DesktopAction) {
        if self.closing {
            return;
        }
        match action {
            DesktopAction::Browse(request) => self.browse(request),
            DesktopAction::RefreshFiles => {
                self.file_jobs.retain(|request, (_, task)| {
                    if matches!(request, crate::project_files::Request::Directory(_)) {
                        task.abort();
                        false
                    } else {
                        true
                    }
                });
                self.app.files.directories.clear();
                self.browse(crate::project_files::Request::Directory(".".into()));
            }
            DesktopAction::CloseFilePreview => {
                self.app.files.preview = None;
                self.file_jobs.retain(|request, (_, task)| {
                    if matches!(
                        request,
                        crate::project_files::Request::File(_)
                            | crate::project_files::Request::Diff(_)
                    ) {
                        task.abort();
                        false
                    } else {
                        true
                    }
                });
            }
            DesktopAction::AttachFile { panel_id, path } => {
                if path.len() > 2048 || path.chars().any(char::is_control) {
                    self.app.notice =
                        "Ez a fájlnév nem csatolható. Használd az ügynök olvasóeszközét.".into();
                    return;
                }
                if !matches!(&self.app.files.preview,Some((crate::project_files::Request::File(current),crate::project_files::Load::Ready(_))) if *current==path)
                {
                    return;
                }
                if let Some(panel) = self.app.panels.iter_mut().find(|panel| {
                    panel.id == panel_id
                        && panel.provider == ProviderKind::OpenRouter
                        && panel.status != PanelStatus::Running
                }) {
                    if panel.attachments.len() < 8 && !panel.attachments.contains(&path) {
                        panel.attachments.push(path);
                    }
                    self.app.files.preview = None;
                }
            }
            DesktopAction::RemoveAttachment { panel_id, path } => {
                if let Some(panel) = self
                    .app
                    .panels
                    .iter_mut()
                    .find(|panel| panel.id == panel_id && panel.status != PanelStatus::Running)
                {
                    panel.attachments.retain(|current| *current != path);
                }
            }
            DesktopAction::ReviewEdit { panel_id, approve } => {
                self.app.edit_reviews.remove(&panel_id);
                if let Some(reply) = self.edit_replies.remove(&panel_id) {
                    let _ = reply.send(approve);
                }
            }
            DesktopAction::Send(id) => self.send(id),
            DesktopAction::Cancel(id) => self.cancel(id),
            DesktopAction::NewPanel => {
                let (model, provider) = self
                    .app
                    .panels
                    .get(self.app.active)
                    .map(|source| (source.model.clone(), source.provider))
                    .unwrap_or_else(|| ("openrouter/auto".into(), ProviderKind::OpenRouter));
                if self.app.add_panel(self.app.next_panel_name(), model) {
                    self.app.active_panel_mut().provider = provider;
                    if provider.is_terminal() {
                        self.start_codex(self.app.active_panel().id);
                    }
                }
            }
            DesktopAction::ClosePanel(id) => {
                if self.app.panels.iter().any(|panel| panel.id == id) {
                    self.app.focus_id(id);
                    // The optional terminal-only frontend keeps one chat; the desktop
                    // supports a genuinely empty workspace after its last panel closes.
                    let closed = if self.app.panels.len() == 1 {
                        self.app.panels.clear();
                        self.app.active = 0;
                        self.app.zoomed = false;
                        Some(id)
                    } else {
                        self.app.close_active()
                    };
                    if let Some(id) = closed {
                        self.stop_worker(id);
                        self.stop_codex(id);
                    }
                }
            }
            DesktopAction::Login => self.begin_login(),
            DesktopAction::CancelLogin => self.cancel_login(),
            DesktopAction::Logout => self.begin_logout(),
            DesktopAction::ConnectCodexAccount => self.begin_native_login(),
            DesktopAction::CancelCodexAccount => self.cancel_native_login(),
            DesktopAction::DisconnectCodexAccount => self.begin_native_logout(),
            DesktopAction::RefreshModels => self.begin_models(),
            DesktopAction::RefreshUsage => self.begin_usage(),
            DesktopAction::RefreshAntigravityUsage => self.begin_antigravity_usage(),
            DesktopAction::SetOpenRouterKey(key) => {
                let key = key.trim();
                if key.is_empty() || key.len() > 8192 || key.chars().any(char::is_control) {
                    self.app.notice =
                        "Érvénytelen OpenRouter API key. A kulcs nem változott.".into();
                } else {
                    self.openrouter_key = key.to_owned();
                    self.app.notice = "OpenRouter API key beállítva erre a munkamenetre.".into();
                }
            }
            DesktopAction::OpenAgent { title, model } => {
                if self.app.add_panel(title, model) {
                    self.app.active_panel_mut().provider = ProviderKind::OpenRouter;
                    self.app.panels[self.app.active].messages.clear();
                }
            }
            DesktopAction::OpenCodex { model } => {
                if !self.app.demo && !self.app.chatgpt_ready {
                    self.app.notice =
                        "Codex CLI indításához jelentkezz be VIBE ChatGPT fiókoddal.".into();
                    return;
                }
                let model = if !model.trim().is_empty() {
                    model
                } else {
                    self.app
                        .available_models
                        .first()
                        .map(|model| model.id.clone())
                        .unwrap_or_else(|| "gpt-6.1-sol".into())
                };
                if self.app.add_panel(self.app.next_panel_name(), model) {
                    self.app.active_panel_mut().provider = ProviderKind::Chatgpt;
                    self.app.panels[self.app.active].messages.clear();
                    self.start_codex(self.app.active_panel().id);
                }
            }
            DesktopAction::RestartCodex(id) => {
                self.stop_codex(id);
                self.start_codex(id);
            }
            DesktopAction::OpenAntigravity => {
                self.app.antigravity_enabled = true;
                if !self.workspace_preferences.antigravity_enabled {
                    let mut preferences = self.workspace_preferences.clone();
                    preferences.antigravity_enabled = true;
                    if let Err(error) = self.update_workspace_preferences(preferences) {
                        self.app.notice =
                            format!("Google-kapcsolat beállítása nem menthető: {error}");
                    }
                }
                // The interactive CLI may select or sign into a different Google account.
                self.cancel_antigravity_usage();
                if self
                    .app
                    .add_panel(self.app.next_panel_name(), "antigravity/default".into())
                {
                    self.app.active_panel_mut().provider = ProviderKind::Antigravity;
                    self.app.active_panel_mut().messages.clear();
                    self.start_codex(self.app.active_panel().id);
                }
            }
            DesktopAction::SetProvider { panel_id, provider } => {
                if self.app.panels.iter().any(|panel| panel.id == panel_id) {
                    self.app.focus_id(panel_id);
                    let previous = self.app.active_panel().provider;
                    if self.app.set_provider(provider) {
                        if previous != provider {
                            self.stop_codex(panel_id);
                        }
                        if provider.is_terminal() {
                            self.start_codex(panel_id);
                        }
                    }
                }
            }
            DesktopAction::SetModel { panel_id, model } => {
                if self.app.panels.iter().any(|panel| panel.id == panel_id) {
                    self.app.focus_id(panel_id);
                    self.app.set_model(model);
                }
            }
            DesktopAction::SetEffort { panel_id, effort } => {
                if self.app.panels.iter().any(|panel| panel.id == panel_id) {
                    self.app.focus_id(panel_id);
                    self.app.set_effort(effort);
                }
            }
            DesktopAction::Save => {
                self.save(true);
            }
            DesktopAction::OpenProject | DesktopAction::CheckUpdates => {}
            DesktopAction::SwitchProject(project) => {
                if let Err(error) = self.switch_project(project) {
                    self.app.notice = error.to_string();
                }
            }
            DesktopAction::SelectProjectPanel { project, panel_id } => {
                if project != self.app.project_path
                    && let Err(error) = self.switch_project(project)
                {
                    self.app.notice = error.to_string();
                    return;
                }
                self.app.focus_id(panel_id);
                if let Some(panel) = self.app.panels.iter().find(|panel| panel.id == panel_id) {
                    self.app.active_group = panel.group.clone();
                }
                self.app.zoomed = true;
            }
            DesktopAction::SetProjectPinned { project, pinned } => {
                if let Err(error) = self.set_project_pinned(project, pinned) {
                    self.app.notice = error.to_string();
                }
            }
            DesktopAction::RemoveProject(project) => {
                if let Err(error) = self.remove_project(project) {
                    self.app.notice = error.to_string();
                }
            }
        }
        self.repaint.request_repaint();
    }

    pub fn switch_project(&mut self, project: PathBuf) -> Result<()> {
        ensure!(!self.closing, "The application is closing.");
        let canonical = project
            .canonicalize()
            .context("Cannot open the project directory")?;
        if canonical == self.app.project_path {
            let mut preferences = self.workspace_preferences.clone();
            preferences.reopen(&canonical);
            preferences.last_project = Some(canonical.clone());
            self.update_workspace_preferences(preferences)?;
            self.refresh_recent_projects();
            return Ok(());
        }
        // Prepare the replacement first: failed validation or locking leaves the current project intact.
        let (mut next, path, lock) = load_project(
            &canonical,
            self.app.demo,
            self.no_save,
            &self.openrouter_key,
        )?;
        let next_terminals =
            restore_saved_terminals(&mut next, &self.repaint, self.app.demo || self.no_save)?;
        self.cancel_all(false);
        if let Some(path) = &self.save_path {
            storage::save(path, &self.app)
                .context("Saving the previous session failed; project switch cancelled")?;
        }
        let mut preferences = self.workspace_preferences.clone();
        preferences.reopen(&canonical);
        preferences.last_project = Some(canonical.clone());
        self.update_workspace_preferences(preferences)?;
        next.account_label = self.app.account_label.clone();
        next.account_usage = self.app.account_usage.clone();
        next.antigravity_usage = self.app.antigravity_usage.clone();
        next.antigravity_enabled = self.app.antigravity_enabled;
        next.chatgpt_ready = self.app.chatgpt_ready;
        next.siwc_ready = self.app.siwc_ready;
        next.siwc_account_label = self.app.siwc_account_label.clone();
        next.native_codex_account = self.app.native_codex_account.clone();
        next.native_codex_busy = self.app.native_codex_busy;
        next.native_codex_login_url = self.app.native_codex_login_url.clone();
        next.auth_busy = self.app.auth_busy;
        next.login_url = self.app.login_url.clone();
        next.available_models = self.app.available_models.clone();
        self.cancel_file_jobs();
        self.app = next;
        self.terminals = next_terminals;
        self.save_path = path;
        self.project_lock = lock;
        self.saved_fingerprint = None;
        self.last_save = Instant::now();
        self.refresh_recent_projects();
        self.queue_startup_terminals();
        self.repaint.request_repaint();
        Ok(())
    }

    fn queue_startup_terminals(&mut self) {
        self.startup_terminals = self
            .app
            .panels
            .iter()
            .filter(|panel| panel.provider.is_terminal() && !self.app.demo)
            .map(|panel| panel.id)
            .collect();
    }

    fn start_queued_terminals(&mut self) {
        if self.closing || self.app.demo {
            return;
        }
        // Wait for both saved account checks before launching Codex: a late
        // account result can otherwise stop a terminal that has just started.
        let codex_ready =
            self.app.chatgpt_ready && !self.app.native_codex_busy && !self.app.auth_busy;
        let pending = std::mem::take(&mut self.startup_terminals);
        for id in pending {
            let Some(panel) = self.app.panels.iter().find(|panel| panel.id == id) else {
                continue;
            };
            if panel.provider == ProviderKind::Chatgpt && !codex_ready {
                self.startup_terminals.push(id);
            } else if panel.provider.is_terminal() {
                // Consume each request once; failures remain available for a
                // manual retry instead of repeatedly spawning on every frame.
                self.start_codex(id);
            }
        }
    }

    pub fn request_close(&mut self) {
        if self.closing {
            return;
        }
        self.closing = true;
        self.cancel_file_jobs();
        self.terminal_save_error = None;
        self.cancel_usage();
        self.cancel_antigravity_usage();
        self.app.notice = "Saving the session and closing connections...".into();
        self.cancel_all(false);
        self.account_generation = self.account_generation.wrapping_add(1);
        self.cancel_native_operations();
        if let Some(task) = self.login.take() {
            task.abort();
            self.retired.push(task);
        }
        if let Some(job) = self.account_job.take() {
            if !job.logout {
                job.task.abort();
            }
            self.retired.push(job.task);
        }
        let tasks = std::mem::take(&mut self.retired);
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        self.shutdown = Some(self.runtime.spawn(async move {
            for task in tasks {
                let _ = task.await;
            }
            auth::finish_pending_refreshes().await;
            publish(&output, &repaint, Event::Shutdown);
        }));
    }

    pub fn closing(&self) -> bool {
        self.closing
    }
    pub fn ready_to_close(&self) -> bool {
        self.ready_to_close
    }

    fn stop_worker(&mut self, id: u64) {
        self.edit_replies.remove(&id);
        self.app.edit_reviews.remove(&id);
        if let Some(worker) = self.workers.remove(&id) {
            worker.task.abort();
            self.retired.push(worker.task);
        }
    }

    fn cancel_file_jobs(&mut self) {
        for (_, (_, task)) in self.file_jobs.drain() {
            task.abort();
        }
    }

    fn browse(&mut self, request: crate::project_files::Request) {
        if self.file_jobs.contains_key(&request) {
            return;
        }
        self.app.files.begin(&request);
        if self.app.demo || self.file_jobs.len() >= 8 {
            self.app.files.complete(
                &request,
                Err(if self.app.demo {
                    "Demó: nincs projektfájl-beolvasás."
                } else {
                    "Túl sok fájlművelet; próbáld újra."
                }
                .into()),
            );
            return;
        }
        if matches!(request, crate::project_files::Request::Directory(_))
            && self.app.files.directories.len() > 128
        {
            self.app.files.complete(
                &request,
                Err("128 betöltött mappa a korlát. Használd a Frissítés gombot.".into()),
            );
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let root = self.app.project_path.clone();
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        let operation = request.clone();
        let task = self.runtime.spawn(async move {
            let result = crate::project_files::load(root.clone(), operation.clone())
                .await
                .map_err(|error| format!("{error:#}"));
            publish(
                &output,
                &repaint,
                Event::Files {
                    root,
                    request: operation,
                    generation,
                    result,
                },
            );
        });
        self.file_jobs.insert(request, (generation, task));
    }

    fn start_codex(&mut self, id: u64) {
        if self.app.demo
            || self.codex.contains_key(&id)
            || self
                .terminals
                .get(&id)
                .is_some_and(|session| !session.is_restored())
        {
            return;
        }
        let Some(panel) = self
            .app
            .panels
            .iter()
            .find(|panel| panel.id == id && panel.provider.is_terminal())
        else {
            return;
        };
        let provider = panel.provider;
        let label = terminal_label(provider);
        if provider == ProviderKind::Chatgpt && self.app.native_codex_busy {
            self.app.notice = "Várd meg a Codex fiókművelet végét.".into();
            return;
        }
        if provider == ProviderKind::Chatgpt && !self.app.chatgpt_ready {
            self.app.notice = "Codex CLI indításához jelentkezz be VIBE ChatGPT fiókoddal.".into();
            return;
        }
        let model = panel.model.clone();
        let effort = panel.effort;
        let native_account = self.app.native_codex_account.is_some();
        let cwd = self.app.project_path.clone();
        let archive = if self.no_save {
            None
        } else {
            let panel = self
                .app
                .panels
                .iter_mut()
                .find(|panel| panel.id == id)
                .unwrap();
            let key = panel
                .terminal_key
                .get_or_insert_with(|| uuid::Uuid::new_v4().to_string());
            match TerminalStore::open(key, provider) {
                Ok(store) => Some(Arc::new(store)),
                Err(error) => {
                    self.app.notice = error.to_string();
                    return;
                }
            }
        };
        // Replace the offline surface only after the account checks succeeded.
        self.terminals.remove(&id);
        if let Some(panel) = self.app.panels.iter_mut().find(|panel| panel.id == id) {
            panel.terminal_saved = false;
        }
        if provider == ProviderKind::Antigravity {
            // Restart and provider changes can also expose the CLI's account picker.
            self.cancel_antigravity_usage();
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        let task = self.runtime.spawn(async move {
            if provider == ProviderKind::Antigravity {
                let progress_output = output.clone();
                let progress_repaint = repaint.clone();
                let last_download = std::sync::Mutex::new(None::<Instant>);
                let install = antigravity::ensure_installed_with_progress(move |progress| {
                    if matches!(&progress, antigravity::InstallProgress::Downloading { .. }) {
                        let Ok(mut last) = last_download.lock() else {
                            return;
                        };
                        if last.is_some_and(|time| time.elapsed() < Duration::from_millis(200)) {
                            return;
                        }
                        *last = Some(Instant::now());
                    }
                    publish(
                        &progress_output,
                        &progress_repaint,
                        Event::TerminalProgress(id, generation, installation_notice(progress)),
                    );
                });
                let installed = tokio::select! {
                    _ = cancelled.changed() => return,
                    result = install => result,
                };
                if let Err(error) = installed {
                    publish(
                        &output,
                        &repaint,
                        Event::Codex(id, generation, Err(error.to_string())),
                    );
                    return;
                }
            }
            let token = if provider == ProviderKind::Chatgpt && !native_account {
                tokio::select! {
                    _ = cancelled.changed() => return,
                    token = auth::access_token() => token.map(Some),
                }
            } else {
                // Google login belongs to the native CLI, including its browser and keyring.
                Ok(None)
            };
            if *cancelled.borrow() {
                return;
            }
            let result = match token {
                Ok(token) => {
                    let terminal_repaint = repaint.clone();
                    // Always reclaim the spawn result so cancelling a panel cannot strand its process.
                    match tokio::task::spawn_blocking(move || match provider {
                        ProviderKind::Chatgpt if native_account => {
                            TerminalSession::spawn_native_persisted(
                                cwd,
                                model,
                                effort,
                                terminal_repaint,
                                archive,
                            )
                        }
                        ProviderKind::Chatgpt => TerminalSession::spawn_persisted(
                            cwd,
                            model,
                            effort,
                            token.expect("SIWC token resolved"),
                            terminal_repaint,
                            archive,
                        ),
                        _ => TerminalSession::spawn_antigravity_persisted(
                            cwd,
                            terminal_repaint,
                            archive,
                        ),
                    })
                    .await
                    {
                        Ok(result) => result.map_err(|error| error.to_string()),
                        Err(_) => Err(format!("{label} indítószála leállt.")),
                    }
                }
                Err(error) => Err(error.to_string()),
            };
            if *cancelled.borrow() {
                return;
            }
            publish(&output, &repaint, Event::Codex(id, generation, result));
        });
        self.codex.insert(
            id,
            CodexPanel {
                generation,
                cancel,
                task,
            },
        );
        if let Some(panel) = self.app.panels.iter_mut().find(|panel| panel.id == id) {
            panel.status = PanelStatus::Running;
        }
        self.app.notice = format!("Eredeti interaktív {label} indul a projekt könyvtárában…");
    }

    pub fn terminal_starting(&self, id: u64) -> bool {
        self.codex.contains_key(&id)
    }

    fn stop_codex(&mut self, id: u64) {
        self.app.terminal_notices.remove(&id);
        if let Some(panel) = self.codex.remove(&id) {
            let _ = panel.cancel.send(true);
            self.retired.push(panel.task);
        }
        // Stop and close ConPTY off the UI thread. Its destructor may wait for
        // the console host to drain output even after the child is killed.
        if let Some(mut session) = self.terminals.remove(&id) {
            let output = self.sender.clone();
            let repaint = self.repaint.clone();
            self.retired.push(self.runtime.spawn_blocking(move || {
                if let Err(error) = session.save_and_stop() {
                    publish(
                        &output,
                        &repaint,
                        Event::TerminalSaveFailed(error.to_string()),
                    );
                }
            }));
        }
    }

    fn retire_terminal(&mut self, mut session: TerminalSession) {
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        self.retired.push(self.runtime.spawn_blocking(move || {
            if let Err(error) = session.save_and_stop() {
                publish(
                    &output,
                    &repaint,
                    Event::TerminalSaveFailed(error.to_string()),
                );
            }
        }));
    }

    fn codex_status(
        &mut self,
        id: u64,
        generation: u64,
        status: std::result::Result<TerminalSession, String>,
    ) {
        if !self
            .codex
            .get(&id)
            .is_some_and(|panel| panel.generation == generation)
        {
            if let Ok(session) = status {
                self.retire_terminal(session);
            }
            return;
        }
        if let Some(panel) = self.codex.remove(&id) {
            self.retired.push(panel.task);
        }
        self.app.terminal_notices.remove(&id);
        let label = self
            .app
            .panels
            .iter()
            .find(|panel| panel.id == id)
            .map(|panel| terminal_label(panel.provider))
            .unwrap_or("CLI");
        match status {
            Ok(session) => {
                self.terminals.insert(id, session);
                if let Some(panel) = self.app.panels.iter_mut().find(|panel| panel.id == id) {
                    panel.status = PanelStatus::Ready;
                }
                self.app.notice = if label == "Codex CLI" {
                    "Codex CLI fut. A munkamenetmentés hookjait egyszer a /hooks menüben hagyd jóvá.".into()
                } else {
                    format!("Eredeti {label} fut. Engedélykérésekre a terminálban válaszolhatsz.")
                };
            }
            Err(message) => {
                self.app.notice = format!("{label} hiba: {message}");
                if let Some(panel) = self.app.panels.iter_mut().find(|panel| panel.id == id) {
                    panel.status = PanelStatus::Error;
                    panel.messages.push(Message {
                        role: Role::System,
                        content: format!("{label} indítási hiba: {message}"),
                    });
                }
                self.restore_saved_panel(id);
            }
        }
    }

    fn restore_saved_panel(&mut self, id: u64) {
        let Some(panel) = self.app.panels.iter_mut().find(|panel| panel.id == id) else {
            return;
        };
        if let Some(key) = &panel.terminal_key
            && let Ok(store) = TerminalStore::open(key, panel.provider)
            && let Ok(Some(archive)) = store.load()
            && let Ok(surface) = TerminalSession::restored(
                archive,
                Arc::new(store),
                panel.model.clone(),
                panel.effort,
                self.repaint.clone(),
            )
        {
            panel.terminal_saved = true;
            self.terminals.insert(id, surface);
        }
    }

    fn cancel(&mut self, id: u64) {
        if self.codex.contains_key(&id) {
            self.stop_codex(id);
        }
        if let Some(terminal) = self.terminals.get(&id) {
            let _ = terminal.write(b"\x03");
        }
        self.stop_worker(id);
        if let Some(panel) = self
            .app
            .panels
            .iter_mut()
            .find(|panel| panel.id == id && panel.status == PanelStatus::Running)
        {
            panel.status = PanelStatus::Cancelled;
            panel.messages.push(Message {
                role: Role::System,
                content: "Response stopped; the remaining text is partial.".into(),
            });
        }
    }

    fn cancel_all(&mut self, chatgpt_only: bool) {
        let ids: Vec<_> = self
            .app
            .panels
            .iter()
            .filter(|panel| !chatgpt_only || panel.provider == ProviderKind::Chatgpt)
            .map(|panel| panel.id)
            .collect();
        for id in ids {
            self.cancel(id);
            self.stop_codex(id);
        }
    }

    fn send(&mut self, id: u64) {
        let Some(panel) = self.app.panels.iter().find(|panel| panel.id == id) else {
            return;
        };
        if panel.status == PanelStatus::Running {
            self.app.notice = "This agent is still responding. Stop it first.".into();
            return;
        }
        let text: String = panel
            .input
            .chars()
            .filter(|character| !character.is_control() || *character == '\n' || *character == '\t')
            .collect();
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        if text.len() > 64 * 1024 {
            self.app.notice = "Messages are limited to 64 KiB. Your draft was kept.".into();
            return;
        }
        if panel.provider == ProviderKind::OpenRouter && text.starts_with('/') {
            let panel = self
                .app
                .panels
                .iter_mut()
                .find(|panel| panel.id == id)
                .expect("panel exists");
            let handled = match text.as_str() {
                "/help" => {
                    panel.messages.push(Message {role:Role::System,content:"VIBE Agent parancsok\n/help – súgó\n/clear – látható beszélgetés törlése\n/model szolgáltató/modell – modellváltás\nA projekt fájljait automatikusan felderíti és olvassa. Írás előtt előnézetet és jóváhagyást kér. Nincs shell- vagy parancsfuttatás. Enter: küldés · Shift+Enter: új sor · Esc: leállítás".into()});
                    true
                }
                "/clear" => {
                    panel.messages.clear();
                    panel.usage = None;
                    true
                }
                "/model" => {
                    panel.messages.push(Message {
                        role: Role::System,
                        content: format!(
                            "Aktív modell: {}\nVáltás: /model szolgáltató/modell",
                            panel.model
                        ),
                    });
                    true
                }
                _ if text.starts_with("/model ") => {
                    let model = text[7..].trim();
                    if !model.is_empty()
                        && model.len() <= 200
                        && !model.chars().any(char::is_whitespace)
                    {
                        panel.model = model.into();
                        panel.messages.push(Message {
                            role: Role::System,
                            content: format!("Modell beállítva: {model}"),
                        });
                    }
                    true
                }
                _ => false,
            };
            if handled {
                panel.input.clear();
                panel.cursor = 0;
                return;
            }
        }
        let panel = self
            .app
            .panels
            .iter()
            .find(|panel| panel.id == id)
            .expect("panel exists");
        if !self.app.demo {
            match panel.provider {
                ProviderKind::Chatgpt if self.plan_limited => {
                    self.app.notice = "ChatGPT usage limit reached. Details: https://chatgpt.com/settings/usage. Your draft was kept.".into();
                    return;
                }
                ProviderKind::Chatgpt if !self.app.chatgpt_ready => {
                    self.app.notice =
                        "Sign in with ChatGPT and allow subscription access. Your draft was kept."
                            .into();
                    return;
                }
                ProviderKind::OpenRouter if self.openrouter_key.trim().is_empty() => {
                    self.app.notice = "OPENROUTER_API_KEY is missing. Choose ChatGPT or set the key. Your draft was kept.".into();
                    return;
                }
                _ => {}
            }
        }
        let connection = panel.provider;
        if !self.app.demo && connection.is_terminal() {
            self.start_codex(id);
            self.app.notice = format!(
                "A {} termináljába írj; az eredeti felület kezeli az üzeneteket.",
                terminal_label(connection)
            );
            return;
        }
        let panel = self
            .app
            .panels
            .iter_mut()
            .find(|panel| panel.id == id)
            .expect("panel exists");
        panel.messages.push(Message {
            role: Role::User,
            content: text.clone(),
        });
        panel.input.clear();
        panel.cursor = 0;
        panel.scroll = 0;
        panel.status = PanelStatus::Running;
        panel.usage = None;
        let request = ChatRequest {
            panel_id: id,
            model: panel.model.clone(),
            messages: panel.messages.clone(),
        };
        panel.messages.push(Message {
            role: Role::Assistant,
            content: String::new(),
        });
        let attachments = panel.attachments.clone();
        self.stop_worker(id);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        let key = self.openrouter_key.clone();
        let demo = self.app.demo;
        let workspace = self.app.project_path.clone();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let task = self.runtime.spawn(async move {
            let inference = async {
                if demo { provider::demo_chat(request, sender).await; }
                else { crate::openrouter_agent::run_with_files(request, workspace, key, sender, attachments).await; }
            };
            tokio::pin!(inference);
            loop {
                tokio::select! {
                    () = &mut inference => {
                        while let Ok(event) = receiver.try_recv() { publish(&output, &repaint, Event::Provider(generation, event)); }
                        break;
                    }
                    Some(event) = receiver.recv() => publish(&output, &repaint, Event::Provider(generation, event)),
                }
            }
        });
        self.workers.insert(id, Worker { generation, task });
    }

    fn provider_event(&mut self, generation: u64, event: ProviderEvent) {
        let id = match &event {
            ProviderEvent::PreparedInput { panel_id, .. } => *panel_id,
            ProviderEvent::Activity { panel_id, .. }
            | ProviderEvent::Review { panel_id, .. }
            | ProviderEvent::Delta { panel_id, .. }
            | ProviderEvent::Usage { panel_id, .. }
            | ProviderEvent::Finished { panel_id }
            | ProviderEvent::Failed { panel_id, .. } => *panel_id,
        };
        if !self
            .workers
            .get(&id)
            .is_some_and(|worker| worker.generation == generation)
        {
            return;
        }
        let Some(panel) = self.app.panels.iter_mut().find(|panel| panel.id == id) else {
            return;
        };
        let mut terminal = false;
        match event {
            ProviderEvent::PreparedInput { content, .. } => {
                if let Some(message) = panel
                    .messages
                    .iter_mut()
                    .rfind(|message| message.role == Role::User)
                {
                    message.content = content;
                }
                panel.attachments.clear();
            }
            ProviderEvent::Activity { text, .. } => {
                self.app.edit_reviews.remove(&id);
                self.edit_replies.remove(&id);
                panel.messages.push(Message {
                    role: Role::System,
                    content: text,
                });
                panel.messages.push(Message {
                    role: Role::Assistant,
                    content: String::new(),
                });
            }
            ProviderEvent::Review { edit, reply, .. } => {
                self.app.edit_reviews.insert(id, edit);
                self.edit_replies.insert(id, reply);
            }
            ProviderEvent::Delta { text, .. } => {
                if let Some(message) = panel
                    .messages
                    .last_mut()
                    .filter(|message| message.role == Role::Assistant)
                {
                    message.content.extend(text.chars().filter(|character| {
                        !character.is_control() || *character == '\n' || *character == '\t'
                    }));
                }
            }
            ProviderEvent::Usage { usage, .. } => panel.usage = Some(usage),
            ProviderEvent::Finished { .. } => {
                panel.status = PanelStatus::Ready;
                crate::completion_sound::play();
                terminal = true;
            }
            ProviderEvent::Failed { message, .. } => {
                panel.status = PanelStatus::Error;
                if !panel.attachments.is_empty()
                    && panel.input.is_empty()
                    && let Some(prompt) = panel
                        .messages
                        .iter()
                        .rfind(|message| message.role == Role::User)
                {
                    panel.input = prompt.content.clone();
                    panel.cursor = panel.input.len();
                }
                if panel.provider == ProviderKind::Chatgpt
                    && message.contains("subscription_sharing_usage_limit_exceeded")
                {
                    self.plan_limited = true;
                }
                panel.messages.push(Message {
                    role: Role::System,
                    content: format!("Request failed: {message}"),
                });
                terminal = true;
            }
        }
        if terminal && let Some(worker) = self.workers.remove(&id) {
            self.app.edit_reviews.remove(&id);
            self.edit_replies.remove(&id);
            self.retired.push(worker.task);
        }
    }

    fn update_live_usage(&mut self) {
        if self.closing || self.app.demo || self.app.native_codex_account.is_none() {
            self.live_usage = Default::default();
            return;
        }
        let revisions =
            self.terminals
                .iter()
                .filter(|(id, terminal)| {
                    !terminal.is_restored()
                        && terminal.exited().is_none()
                        && self.app.panels.iter().any(|panel| {
                            panel.id == **id && panel.provider == ProviderKind::Chatgpt
                        })
                })
                .map(|(id, terminal)| (*id, terminal.output_revision()))
                .collect();
        if self.live_usage.observe(
            revisions,
            Instant::now(),
            self.usage_last_attempt,
            self.usage_job.is_none() && !self.app.native_codex_busy,
        ) {
            self.begin_usage();
        }
    }

    fn begin_usage(&mut self) {
        if self.app.demo {
            self.app.account_usage.error = Some("Demó módban nincs valódi használati adat.".into());
            return;
        }
        if self.app.native_codex_account.is_none() {
            self.app.account_usage.error = Some(
                "A használati adatokhoz csatlakoztasd saját Codex fiókodat a profilban.".into(),
            );
            return;
        }
        if self.app.native_codex_busy || self.usage_job.is_some() {
            return;
        }
        self.usage_generation = self.usage_generation.wrapping_add(1);
        let generation = self.usage_generation;
        self.usage_last_attempt = Some(Instant::now());
        self.app.account_usage.refreshing = true;
        self.app.account_usage.error = None;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        self.usage_job = Some(self.runtime.spawn(async move {
            let result = account_usage::fetch()
                .await
                .map_err(|error| error.to_string());
            publish(&output, &repaint, Event::Usage(generation, result));
        }));
    }

    fn begin_antigravity_usage(&mut self) {
        if self.app.demo {
            self.app.antigravity_usage.error =
                Some("Demó módban nincs valódi Antigravity használati adat.".into());
            return;
        }
        if self.antigravity_usage_job.is_some() {
            return;
        }
        // A usage query must never compete with this controller's CLI installation.
        if self.app.panels.iter().any(|panel| {
            panel.provider == ProviderKind::Antigravity && self.codex.contains_key(&panel.id)
        }) {
            self.app.antigravity_usage.refreshing = false;
            self.app.antigravity_usage.error =
                Some("Várd meg az Antigravity CLI telepítésének és indításának végét.".into());
            return;
        }
        self.antigravity_usage_generation = self.antigravity_usage_generation.wrapping_add(1);
        let generation = self.antigravity_usage_generation;
        self.antigravity_usage_last_attempt = Some(Instant::now());
        self.app.antigravity_usage.refreshing = true;
        self.app.antigravity_usage.error = None;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        self.antigravity_usage_job = Some(self.runtime.spawn(async move {
            let result = antigravity_usage::fetch_usage()
                .await
                .map_err(|error| error.to_string());
            publish(
                &output,
                &repaint,
                Event::AntigravityUsage(generation, result),
            );
        }));
    }

    fn cancel_antigravity_usage(&mut self) {
        self.antigravity_usage_generation = self.antigravity_usage_generation.wrapping_add(1);
        if let Some(task) = self.antigravity_usage_job.take() {
            task.abort();
            self.retired.push(task);
        }
        self.antigravity_usage_last_attempt = None;
        self.app.antigravity_usage = Default::default();
    }

    fn antigravity_usage_event(
        &mut self,
        generation: u64,
        result: std::result::Result<AntigravityUsage, String>,
    ) {
        if generation != self.antigravity_usage_generation {
            return;
        }
        if let Some(task) = self.antigravity_usage_job.take() {
            self.retired.push(task);
        }
        self.app.antigravity_usage.refreshing = false;
        match result {
            Ok(snapshot) => {
                self.app.antigravity_usage.snapshot = Some(snapshot);
                self.app.antigravity_usage.error = None;
            }
            // Preserve the sample as stale; an error never means zero quota.
            Err(message) => self.app.antigravity_usage.error = Some(message),
        }
    }

    fn cancel_usage(&mut self) {
        self.usage_generation = self.usage_generation.wrapping_add(1);
        if let Some(task) = self.usage_job.take() {
            task.abort();
            self.retired.push(task);
        }
        self.usage_last_attempt = None;
        self.app.account_usage = Default::default();
    }

    fn usage_event(&mut self, generation: u64, result: std::result::Result<AccountUsage, String>) {
        if generation != self.usage_generation || self.app.native_codex_account.is_none() {
            return;
        }
        if let Some(task) = self.usage_job.take() {
            self.retired.push(task);
        }
        self.app.account_usage.refreshing = false;
        match result {
            Ok(snapshot) => {
                self.app.account_usage.snapshot = Some(snapshot);
                self.app.account_usage.error = None;
            }
            // Preserve the last sample so the UI can mark it stale, alongside the error.
            Err(message) => self.app.account_usage.error = Some(message),
        }
    }

    fn recompute_account(&mut self) {
        self.app.chatgpt_ready = self.app.siwc_ready || self.app.native_codex_account.is_some();
        self.app.account_label = self
            .app
            .native_codex_account
            .as_ref()
            .map(|info| info.email.clone().unwrap_or_else(|| "Codex account".into()))
            .unwrap_or_else(|| self.app.siwc_account_label.clone());
        if !self.app.chatgpt_ready {
            self.app.available_models.clear();
        }
    }

    fn set_native_account(&mut self, info: Option<codex_account::AccountInfo>) {
        if self.app.native_codex_account != info {
            self.cancel_all(true);
            self.cancel_usage();
        }
        self.app.native_codex_account = info;
        self.plan_limited = false;
        self.recompute_account();
    }

    fn cancel_native_operations(&mut self) {
        self.native_generation = self.native_generation.wrapping_add(1);
        if let Some(task) = self.native_login.take() {
            task.abort();
            self.retired.push(task);
        }
        if let Some(job) = self.native_job.take() {
            if !job.logout {
                job.task.abort();
            }
            self.retired.push(job.task);
        }
        self.app.native_codex_busy = false;
        self.app.native_codex_login_url = None;
    }

    fn finish_native_job(&mut self) {
        if let Some(job) = self.native_job.take() {
            self.retired.push(job.task);
        }
        self.app.native_codex_busy = false;
    }

    fn begin_native_status(&mut self) {
        self.cancel_native_operations();
        self.app.native_codex_busy = true;
        let generation = self.native_generation;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        let task = self.runtime.spawn(async move {
            let result = codex_account::saved_account()
                .await
                .map_err(|error| error.to_string());
            publish(&output, &repaint, Event::NativeAccount(generation, result));
        });
        self.native_job = Some(AccountJob {
            task,
            logout: false,
        });
    }

    fn begin_native_login(&mut self) {
        if self.app.demo {
            self.app.notice = "Demó módban nincs valódi Codex bejelentkezés.".into();
            return;
        }
        if self.native_job.as_ref().is_some_and(|job| job.logout) {
            self.app.notice = "Várd meg a Codex kijelentkezés végét.".into();
            return;
        }
        self.cancel_native_operations();
        self.cancel_all(true);
        self.cancel_usage();
        self.app.native_codex_busy = true;
        self.app.notice = "Saját Codex fiók csatlakoztatása…".into();
        let generation = self.native_generation;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        self.native_login = Some(self.runtime.spawn(async move {
            let (sender, mut receiver) = mpsc::unbounded_channel();
            let login = codex_account::login(sender);
            tokio::pin!(login);
            loop {
                tokio::select! {
                    () = &mut login => {
                        while let Ok(event) = receiver.try_recv() {
                            publish(&output, &repaint, Event::NativeLogin(generation, event));
                        }
                        break;
                    }
                    Some(event) = receiver.recv() => publish(&output, &repaint, Event::NativeLogin(generation, event)),
                }
            }
        }));
    }

    fn cancel_native_login(&mut self) {
        if self.native_login.is_none() {
            return;
        }
        self.cancel_native_operations();
        self.cancel_usage();
        self.app.notice = "Codex bejelentkezés megszakítva.".into();
        self.begin_native_status();
    }

    fn begin_native_logout(&mut self) {
        if self.app.demo {
            self.app.notice = "Demó módban nincs valódi Codex kijelentkezés.".into();
            return;
        }
        if self.native_job.as_ref().is_some_and(|job| job.logout) {
            return;
        }
        self.cancel_native_operations();
        self.cancel_all(true);
        self.cancel_usage();
        self.set_native_account(None);
        self.app.native_codex_busy = true;
        let generation = self.native_generation;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        let task = self.runtime.spawn(async move {
            let result = codex_account::logout()
                .await
                .map_err(|error| error.to_string());
            publish(&output, &repaint, Event::NativeLogout(generation, result));
        });
        self.native_job = Some(AccountJob { task, logout: true });
    }

    fn native_login_event(&mut self, event: codex_account::LoginEvent) {
        match event {
            codex_account::LoginEvent::Waiting { authorization_url } => {
                self.app.native_codex_login_url = Some(authorization_url);
            }
            codex_account::LoginEvent::Completed(info) => {
                if let Some(task) = self.native_login.take() {
                    self.retired.push(task);
                }
                self.cancel_all(true);
                self.cancel_usage();
                self.app.native_codex_busy = false;
                self.app.native_codex_login_url = None;
                self.set_native_account(Some(info));
                self.app.notice = "Saját Codex fiók csatlakoztatva. A terminál és a limitek ezt a fiókot használják.".into();
            }
            codex_account::LoginEvent::Failed(message) => {
                if let Some(task) = self.native_login.take() {
                    self.retired.push(task);
                }
                self.app.native_codex_busy = false;
                self.app.native_codex_login_url = None;
                self.app.notice = message;
                self.begin_native_status();
            }
        }
    }

    fn begin_status(&mut self) {
        self.app.auth_busy = true;
        self.account_generation = self.account_generation.wrapping_add(1);
        let generation = self.account_generation;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        let task = self.runtime.spawn(async move {
            let result = auth::account().await.map_err(|error| error.to_string());
            publish(
                &output,
                &repaint,
                Event::Account(generation, AccountResult::Status(result)),
            );
        });
        self.account_job = Some(AccountJob {
            task,
            logout: false,
        });
    }

    fn begin_login(&mut self) {
        if self.app.demo {
            self.app.notice = "Demo mode has no network sign-in.".into();
            return;
        }
        if self.app.auth_busy {
            self.app.notice = "An account operation is in progress.".into();
            return;
        }
        if self.app.native_codex_account.is_none() {
            self.cancel_all(true);
        }
        self.app.auth_busy = true;
        self.app.siwc_ready = false;
        self.recompute_account();
        self.app.notice = "Preparing ChatGPT sign-in...".into();
        self.account_generation = self.account_generation.wrapping_add(1);
        let generation = self.account_generation;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        self.login = Some(self.runtime.spawn(async move {
            let (sender, mut receiver) = mpsc::unbounded_channel();
            let login = auth::login(sender); tokio::pin!(login);
            loop {
                tokio::select! {
                    () = &mut login => {
                        while let Ok(event) = receiver.try_recv() { publish(&output, &repaint, Event::Login(generation, event)); }
                        break;
                    }
                    Some(event) = receiver.recv() => publish(&output, &repaint, Event::Login(generation, event)),
                }
            }
        }));
    }

    fn cancel_login(&mut self) {
        if let Some(task) = self.login.take() {
            task.abort();
            self.retired.push(task);
            self.account_generation = self.account_generation.wrapping_add(1);
            self.app.login_url = None;
            self.app.auth_busy = false;
            self.app.notice = "Sign-in attempt cancelled.".into();
            if !self.app.demo {
                self.begin_status();
            }
        }
    }

    fn begin_models(&mut self) {
        if self.app.demo {
            self.app.notice = "Demo mode has no network model request.".into();
            return;
        }
        if self.app.auth_busy {
            self.app.notice = "An account operation is in progress.".into();
            return;
        }
        if !self.app.siwc_ready {
            self.app.notice = "Sign in with ChatGPT before loading models.".into();
            return;
        }
        self.app.auth_busy = true;
        self.account_generation = self.account_generation.wrapping_add(1);
        let generation = self.account_generation;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        let task = self.runtime.spawn(async move {
            let result = chatgpt::list_models()
                .await
                .map_err(|error| error.to_string());
            publish(
                &output,
                &repaint,
                Event::Account(generation, AccountResult::Models(result)),
            );
        });
        self.account_job = Some(AccountJob {
            task,
            logout: false,
        });
    }

    fn begin_logout(&mut self) {
        if self.app.demo {
            self.app.notice = "Demo mode has no real sign-out.".into();
            return;
        }
        if self.app.auth_busy {
            self.app.notice = "Finish the account operation first.".into();
            return;
        }
        if self.app.native_codex_account.is_none() {
            self.cancel_all(true);
        }
        self.app.auth_busy = true;
        self.app.siwc_ready = false;
        self.recompute_account();
        self.app.available_models.clear();
        self.account_generation = self.account_generation.wrapping_add(1);
        let generation = self.account_generation;
        let output = self.sender.clone();
        let repaint = self.repaint.clone();
        let task = self.runtime.spawn(async move {
            let result = auth::logout().await.map_err(|error| error.to_string());
            publish(
                &output,
                &repaint,
                Event::Account(generation, AccountResult::Logout(result)),
            );
        });
        self.account_job = Some(AccountJob { task, logout: true });
    }

    fn set_account(&mut self, info: Option<auth::AccountInfo>) {
        self.app.siwc_ready = info.as_ref().is_some_and(|info| info.plan_usage);
        self.app.siwc_account_label = info
            .map(|info| info.email.unwrap_or_else(|| "ChatGPT account".into()))
            .unwrap_or_else(|| "Nincs bejelentkezve".into());
        self.recompute_account();
    }

    fn login_event(&mut self, event: LoginEvent) {
        match event {
            LoginEvent::Waiting { authorization_url } => {
                self.app.login_url = Some(authorization_url)
            }
            LoginEvent::Completed(info) => {
                if let Some(task) = self.login.take() {
                    self.retired.push(task);
                }
                self.app.login_url = None;
                self.app.auth_busy = false;
                self.plan_limited = false;
                self.set_account(Some(info));
                if self.app.siwc_ready {
                    // Explicit account login makes the active conversation use that account.
                    // A source change must not leave its previous stream writing into the panel.
                    if self
                        .app
                        .panels
                        .get(self.app.active)
                        .is_some_and(|panel| panel.provider != ProviderKind::Antigravity)
                    {
                        let id = self.app.active_panel().id;
                        if self.app.native_codex_account.is_none() {
                            self.cancel(id);
                        }
                        self.app.set_provider(ProviderKind::Chatgpt);
                    }
                    self.app.notice = "ChatGPT sign-in complete. Loading models...".into();
                    self.begin_models();
                } else {
                    self.app.notice = "Sign-in complete, but subscription access is not enabled. Sign in again and allow it.".into();
                }
            }
            LoginEvent::Failed(message) => {
                if let Some(task) = self.login.take() {
                    self.retired.push(task);
                }
                self.app.login_url = None;
                self.app.auth_busy = false;
                self.app.notice = message;
                self.begin_status();
            }
        }
    }

    fn account_event(&mut self, result: AccountResult) {
        if let Some(job) = self.account_job.take() {
            self.retired.push(job.task);
        }
        self.app.auth_busy = false;
        match result {
            AccountResult::Status(Ok(info)) => {
                self.set_account(info);
                // Reading a saved account must not trigger a model HTTP request.
            }
            AccountResult::Models(Ok(models)) => {
                self.app.available_models = models
                    .into_iter()
                    .map(|model| ModelChoice {
                        id: model.id,
                        name: model.name,
                    })
                    .collect();
                self.app.model_selection = 0;
                if let Some(first) = self.app.available_models.first() {
                    for panel in &mut self.app.panels {
                        if panel.provider == ProviderKind::Chatgpt
                            && panel.status != PanelStatus::Running
                            && !self
                                .app
                                .available_models
                                .iter()
                                .any(|model| model.id == panel.model)
                        {
                            panel.model = first.id.clone();
                        }
                    }
                    self.app.notice =
                        "ChatGPT connected. Choose a model in the agent header.".into();
                } else {
                    self.app.notice =
                        "No selectable model arrived for this account. Refresh the model list."
                            .into();
                }
            }
            AccountResult::Logout(Ok(revoked)) => {
                self.set_account(None);
                self.plan_limited = false;
                self.app.notice = if revoked {
                    "ChatGPT kijelentkezve.".into()
                } else {
                    "Local sign-in removed; remote revocation was not confirmed. ChatGPT settings: disconnect.".into()
                };
            }
            AccountResult::Status(Err(message))
            | AccountResult::Models(Err(message))
            | AccountResult::Logout(Err(message)) => self.app.notice = message,
        }
    }

    fn save(&mut self, explicit: bool) -> bool {
        self.last_save = Instant::now();
        let Some(path) = &self.save_path else {
            if explicit {
                self.app.notice = "Local saving is disabled for this session.".into();
            }
            return true;
        };
        let fingerprint = fingerprint(&self.app);
        if !explicit && self.saved_fingerprint == Some(fingerprint) {
            return true;
        }
        match storage::save(path, &self.app) {
            Ok(()) => {
                self.saved_fingerprint = Some(fingerprint);
                if explicit {
                    self.app.notice = "Session saved.".into();
                }
                true
            }
            Err(error) => {
                self.app.notice = format!("Saving the session failed: {error}");
                false
            }
        }
    }
}

impl Drop for DesktopController {
    fn drop(&mut self) {
        self.cancel_file_jobs();
        self.antigravity_usage_generation = self.antigravity_usage_generation.wrapping_add(1);
        if let Some(task) = &self.antigravity_usage_job {
            task.abort();
        }
        if let Some(task) = &self.usage_job {
            task.abort();
        }
        for worker in self.workers.values() {
            worker.task.abort();
        }
        if let Some(task) = &self.login {
            task.abort();
        }
        if let Some(task) = &self.native_login {
            task.abort();
        }
        if let Some(job) = &self.native_job
            && !job.logout
        {
            job.task.abort();
        }
        for panel in self.codex.values() {
            let _ = panel.cancel.send(true);
        }
        if let Some(job) = &self.account_job
            && !job.logout
        {
            job.task.abort();
        }
    }
}

fn terminal_label(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Antigravity => "Antigravity CLI",
        ProviderKind::Chatgpt => "Codex CLI",
        ProviderKind::OpenRouter => "CLI",
    }
}

fn installation_notice(progress: antigravity::InstallProgress) -> String {
    match progress {
        antigravity::InstallProgress::Checking => "Antigravity CLI ellenőrzése…".into(),
        antigravity::InstallProgress::WaitingForInstaller => {
            "Várakozás az Antigravity telepítőre…".into()
        }
        antigravity::InstallProgress::FetchingManifest => {
            "Hivatalos kiadás adatainak lekérése…".into()
        }
        antigravity::InstallProgress::Downloading { downloaded, total } => match total {
            Some(total) if total > 0 => format!(
                "Antigravity letöltése: {}%",
                downloaded.saturating_mul(100) / total
            ),
            _ => format!("Antigravity letöltése: {} KiB", downloaded / 1024),
        },
        antigravity::InstallProgress::Verifying => "Antigravity csomag ellenőrzése…".into(),
        antigravity::InstallProgress::Installing => "Antigravity CLI telepítése…".into(),
        antigravity::InstallProgress::Ready => "Antigravity CLI indítása…".into(),
    }
}

fn publish(sender: &mpsc::UnboundedSender<Event>, repaint: &egui::Context, event: Event) {
    if sender.send(event).is_ok() {
        repaint.request_repaint();
    }
}

fn restore_saved_terminals(
    app: &mut App,
    repaint: &egui::Context,
    disabled: bool,
) -> Result<HashMap<u64, TerminalSession>> {
    restore_saved_terminals_with(app, repaint, disabled, |key, provider| {
        Ok(Arc::new(TerminalStore::open(key, provider)?))
    })
}

fn restore_saved_terminals_with(
    app: &mut App,
    repaint: &egui::Context,
    disabled: bool,
    mut open: impl FnMut(&str, ProviderKind) -> Result<Arc<TerminalStore>>,
) -> Result<HashMap<u64, TerminalSession>> {
    let mut terminals = HashMap::new();
    if disabled {
        return Ok(terminals);
    }
    for panel in &mut app.panels {
        if let Some(key) = &panel.terminal_key
            && panel.provider.is_terminal()
        {
            let store = open(key, panel.provider)?;
            if let Some(archive) = store.load()? {
                terminals.insert(
                    panel.id,
                    TerminalSession::restored(
                        archive,
                        store,
                        panel.model.clone(),
                        panel.effort,
                        repaint.clone(),
                    )?,
                );
                panel.terminal_saved = true;
            }
        }
    }
    Ok(terminals)
}

fn load_project(
    project: &Path,
    demo: bool,
    no_save: bool,
    key: &str,
) -> Result<(App, Option<PathBuf>, Option<File>)> {
    let project = project
        .canonicalize()
        .context("Cannot open the project directory")?;
    ensure!(project.is_dir(), "The project path must be a directory.");
    let mut app = App::new(project, demo);
    if !demo {
        app.panels.clear();
        app.active = 0;
        app.next_id = 1;
    }
    if !demo && key.trim().is_empty() {
        for index in 0..app.panels.len() {
            app.active = index;
            app.set_provider(ProviderKind::Chatgpt);
        }
        app.active = 0;
    }
    app.notice = if demo {
        "DEMO - Local sample replies, no network requests".into()
    } else {
        "Válassz agentet. ChatGPT bejelentkezés bal alul; Codex CLI indítás az agentválasztóban."
            .into()
    };
    if demo || no_save {
        return Ok((app, None, None));
    }
    let path = storage::session_path(&app.project_path)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.create(true).truncate(false).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let lock = options.open(path.with_extension("lock"))?;
    lock.try_lock_exclusive()
        .context("This project is already open. Disable saving for a second instance.")?;
    if let Some(session) = storage::load(&path, &app.project_path)? {
        app.panels = session.panels;
        app.workspace_groups = session.workspace_groups;
        app.active_group = session.active_group;
        app.active = session.active;
        app.layout = session.layout;
        app.sidebar_visible = session.sidebar_visible;
        app.next_id = session.next_id;
        app.notice = "Local session restored.".into();
    }
    Ok((app, Some(path), Some(lock)))
}

fn fingerprint(app: &App) -> u64 {
    let mut hash = DefaultHasher::new();
    app.workspace_groups.hash(&mut hash);
    app.active_group.hash(&mut hash);
    app.active.hash(&mut hash);
    (app.layout as u8).hash(&mut hash);
    app.sidebar_visible.hash(&mut hash);
    app.next_id.hash(&mut hash);
    for panel in &app.panels {
        panel.id.hash(&mut hash);
        panel.title.hash(&mut hash);
        panel.description.hash(&mut hash);
        panel.group.hash(&mut hash);
        panel.model.hash(&mut hash);
        panel.terminal_key.hash(&mut hash);
        panel.effort.reasoning().hash(&mut hash);
        (panel.provider as u8).hash(&mut hash);
        panel.input.hash(&mut hash);
        panel.attachments.hash(&mut hash);
        (panel.status == PanelStatus::Running).hash(&mut hash);
        panel
            .usage
            .map(|usage| (usage.prompt_tokens, usage.completion_tokens))
            .hash(&mut hash);
        panel.messages.len().hash(&mut hash);
        for message in &panel.messages {
            (message.role as u8).hash(&mut hash);
            message.content.hash(&mut hash);
        }
    }
    hash.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicitly_selected_new_project_has_no_scaffold_panels_or_generated_files() {
        let directory = tempfile::tempdir().unwrap();
        for key in ["", "fake-openrouter-key"] {
            let (mut app, save, lock) = load_project(directory.path(), false, true, key).unwrap();
            assert!(app.panels.is_empty());
            assert!(save.is_none());
            assert!(lock.is_none());
            assert_eq!(app.next_id, 1);
            assert_eq!(app.project_path, directory.path().canonicalize().unwrap());
            assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
            assert!(app.add_panel("Chosen agent".into(), "test/model".into()));
            assert_eq!(app.panels.len(), 1);
            assert_eq!(app.panels[0].id, 1);
        }
    }

    #[test]
    fn startup_queues_all_native_chats_and_waits_for_account() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        for panel in &mut control.app.panels {
            panel.provider = ProviderKind::Chatgpt;
        }
        control.app.chatgpt_ready = false;
        control.queue_startup_terminals();
        assert_eq!(control.startup_terminals.len(), control.app.panels.len());
        let expected = control.startup_terminals.clone();
        control.start_queued_terminals();
        assert_eq!(control.startup_terminals, expected);
        assert!(control.codex.is_empty());
        control.app.panels.clear();
        control.start_queued_terminals();
        assert!(control.startup_terminals.is_empty());
        control.app.demo = true;
    }

    fn antigravity_usage_sample(remaining_percent: Option<f64>) -> AntigravityUsage {
        AntigravityUsage {
            models: vec![antigravity_usage::ModelQuota {
                name: "Gemini sample".into(),
                remaining_percent,
                remaining_amount: None,
                reset_at: Some(2000),
                reset_label: None,
            }],
            fetched_at: 1000,
            account_label: Some("google@example.invalid".into()),
            plan_label: Some("Sample plan".into()),
        }
    }

    #[test]
    fn antigravity_usage_stays_idle_until_manual_refresh_and_demo_never_queries() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.action(DesktopAction::RefreshAntigravityUsage);
        assert!(control.antigravity_usage_job.is_none());
        assert!(control.app.antigravity_usage.error.is_some());
        assert!(control.app.antigravity_usage.snapshot.is_none());
        assert!(control.usage_job.is_none());

        control.app.demo = false;
        control.app.antigravity_usage = Default::default();
        control.poll();
        assert!(control.antigravity_usage_job.is_none());
        assert!(control.antigravity_usage_last_attempt.is_none());
        assert!(control.app.antigravity_usage.snapshot.is_none());
        assert!(control.app.antigravity_usage.error.is_none());
    }

    #[test]
    fn antigravity_usage_error_keeps_sample_and_stops_automatic_retry() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        control.antigravity_usage_last_attempt = Some(Instant::now() - Duration::from_secs(61));
        let generation = control.antigravity_usage_generation;
        control
            .sender
            .send(Event::AntigravityUsage(
                generation,
                Ok(antigravity_usage_sample(Some(42.0))),
            ))
            .unwrap();
        control
            .sender
            .send(Event::AntigravityUsage(
                generation,
                Err("Google sign-in required".into()),
            ))
            .unwrap();
        control.poll();
        assert_eq!(
            control
                .app
                .antigravity_usage
                .snapshot
                .as_ref()
                .unwrap()
                .models[0]
                .remaining_percent,
            Some(42.0)
        );
        assert_eq!(
            control.app.antigravity_usage.error.as_deref(),
            Some("Google sign-in required")
        );
        assert!(!control.app.antigravity_usage.refreshing);
        assert!(control.antigravity_usage_job.is_none());
        assert!(control.app.native_codex_account.is_none());
        assert!(!control.app.chatgpt_ready);
    }

    #[test]
    fn opening_antigravity_clears_usage_and_rejects_old_result() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let generation = control.antigravity_usage_generation;
        control.antigravity_usage_event(generation, Ok(antigravity_usage_sample(Some(75.0))));
        control.app.antigravity_usage.refreshing = true;
        control.antigravity_usage_job = Some(runtime.spawn(std::future::pending::<()>()));
        control.action(DesktopAction::OpenAntigravity);
        control
            .sender
            .send(Event::AntigravityUsage(
                generation,
                Ok(antigravity_usage_sample(Some(99.0))),
            ))
            .unwrap();
        control.poll();
        assert!(control.app.antigravity_usage.snapshot.is_none());
        assert!(!control.app.antigravity_usage.refreshing);
        assert!(control.app.antigravity_usage.error.is_none());
        assert!(control.antigravity_usage_job.is_none());
        assert!(control.antigravity_usage_last_attempt.is_none());
    }

    #[test]
    fn cached_google_usage_stays_idle_and_manual_query_is_single_flight() {
        let directory = tempfile::tempdir().unwrap();
        // Tasks stay unpolled: this test must never execute the installed CLI.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        control.antigravity_usage_event(
            control.antigravity_usage_generation,
            Ok(antigravity_usage_sample(None)),
        );
        control.antigravity_usage_last_attempt = Some(Instant::now() - Duration::from_secs(59));
        control.poll();
        assert!(control.antigravity_usage_job.is_none());
        control.antigravity_usage_last_attempt = Some(Instant::now() - Duration::from_secs(61));
        control.poll();
        assert!(control.antigravity_usage_job.is_none());
        control.action(DesktopAction::RefreshAntigravityUsage);
        assert!(control.antigravity_usage_job.is_some());
        assert!(control.app.antigravity_usage.refreshing);
        let generation = control.antigravity_usage_generation;
        control.action(DesktopAction::RefreshAntigravityUsage);
        assert_eq!(control.antigravity_usage_generation, generation);
        assert_eq!(
            control
                .app
                .antigravity_usage
                .snapshot
                .as_ref()
                .unwrap()
                .models[0]
                .remaining_percent,
            None
        );
    }

    #[test]
    fn antigravity_usage_waits_for_cli_startup_without_spawning_query() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        control.app.active_panel_mut().provider = ProviderKind::Antigravity;
        let id = control.app.active_panel().id;
        let (cancel, _) = tokio::sync::watch::channel(false);
        control.codex.insert(
            id,
            CodexPanel {
                generation: 10,
                cancel,
                task: runtime.spawn(std::future::pending::<()>()),
            },
        );
        control.action(DesktopAction::RefreshAntigravityUsage);
        assert!(control.antigravity_usage_job.is_none());
        assert!(!control.app.antigravity_usage.refreshing);
        assert!(
            control
                .app
                .antigravity_usage
                .error
                .as_ref()
                .unwrap()
                .contains("Várd meg")
        );
        assert!(control.antigravity_usage_last_attempt.is_none());
    }

    #[test]
    fn closing_cancels_antigravity_usage_and_ignores_queued_result() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        let generation = control.antigravity_usage_generation;
        control.antigravity_usage_event(generation, Ok(antigravity_usage_sample(Some(75.0))));
        control.antigravity_usage_job = Some(runtime.spawn(std::future::pending::<()>()));
        control.app.antigravity_usage.refreshing = true;
        control.request_close();
        control
            .sender
            .send(Event::AntigravityUsage(
                generation,
                Ok(antigravity_usage_sample(Some(1.0))),
            ))
            .unwrap();
        control.poll();
        assert!(control.antigravity_usage_job.is_none());
        assert!(control.app.antigravity_usage.snapshot.is_none());
        assert!(!control.app.antigravity_usage.refreshing);
        assert_ne!(control.antigravity_usage_generation, generation);
    }

    fn usage_sample(remaining_percent: f64) -> AccountUsage {
        AccountUsage {
            primary: Some(account_usage::UsageWindow {
                remaining_percent,
                window_seconds: Some(18000),
                resets_at: Some(2000),
            }),
            secondary: None,
            plan_type: Some("plus".into()),
            fetched_at: 1000,
        }
    }

    fn native_account(email: &str) -> codex_account::AccountInfo {
        codex_account::AccountInfo {
            email: Some(email.into()),
            plan_type: Some("plus".into()),
        }
    }

    #[test]
    fn idle_window_never_spawns_usage_workers_even_with_saved_accounts() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        control.set_native_account(Some(native_account("saved@example.invalid")));
        control.app.antigravity_enabled = true;
        control.app.antigravity_usage.snapshot = Some(antigravity_usage_sample(Some(80.0)));
        control.usage_last_attempt = Some(Instant::now() - Duration::from_secs(600));
        control.antigravity_usage_last_attempt = Some(Instant::now() - Duration::from_secs(600));
        for _ in 0..100 {
            control.poll();
        }
        assert!(control.usage_job.is_none());
        assert!(control.antigravity_usage_job.is_none());
        assert!(control.codex.is_empty());
        control.account_event(AccountResult::Status(Ok(Some(auth::AccountInfo {
            email: None,
            subject: "synthetic".into(),
            plan_usage: true,
        }))));
        assert!(
            control.account_job.is_none(),
            "Saved SIWC account must not fetch models"
        );
    }

    #[test]
    fn usage_late_result_cannot_restore_signed_out_account() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.set_native_account(Some(native_account("own@example.invalid")));
        let generation = control.usage_generation;
        control.usage_event(generation, Ok(usage_sample(72.0)));
        assert!(control.app.account_usage.snapshot.is_some());
        control.app.account_usage.refreshing = true;
        control.set_native_account(None);
        control
            .sender
            .send(Event::Usage(generation, Ok(usage_sample(99.0))))
            .unwrap();
        control.poll();
        assert!(control.app.account_usage.snapshot.is_none());
        assert!(!control.app.account_usage.refreshing);
        assert!(control.app.account_usage.error.is_none());
        assert!(!control.app.chatgpt_ready);
    }

    #[test]
    fn usage_failures_preserve_previous_sample_and_account_switch_clears_it() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.set_native_account(Some(native_account("first@example.invalid")));
        let generation = control.usage_generation;
        control.usage_event(generation, Ok(usage_sample(40.0)));
        control.usage_event(generation, Err("HTTP 403".into()));
        assert_eq!(
            control
                .app
                .account_usage
                .snapshot
                .as_ref()
                .unwrap()
                .primary
                .as_ref()
                .unwrap()
                .remaining_percent,
            40.0
        );
        assert_eq!(control.app.account_usage.error.as_deref(), Some("HTTP 403"));
        control.set_native_account(Some(native_account("second@example.invalid")));
        assert!(control.app.account_usage.snapshot.is_none());
        assert!(control.app.account_usage.error.is_none());
        control.usage_event(generation, Ok(usage_sample(99.0)));
        assert!(control.app.account_usage.snapshot.is_none());
    }

    #[test]
    fn usage_refresh_in_demo_or_signed_out_mode_never_loads_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.action(DesktopAction::RefreshUsage);
        assert!(control.usage_job.is_none());
        assert!(control.app.account_usage.error.is_some());
        control.app.demo = false;
        control.action(DesktopAction::RefreshUsage);
        assert!(control.usage_job.is_none());
        assert!(!control.app.account_usage.refreshing);
        assert!(control.app.account_usage.snapshot.is_none());
    }

    #[test]
    fn siwc_login_cannot_load_native_usage_or_replace_native_identity() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        let siwc = auth::AccountInfo {
            email: Some("siwc@example.invalid".into()),
            subject: "siwc-user".into(),
            plan_usage: true,
        };
        control.set_account(Some(siwc.clone()));
        control.begin_usage();
        assert!(control.usage_job.is_none());
        assert!(
            control
                .app
                .account_usage
                .error
                .as_ref()
                .unwrap()
                .contains("Codex")
        );
        control.set_native_account(Some(native_account("native@example.invalid")));
        control.usage_event(control.usage_generation, Ok(usage_sample(62.0)));
        control.set_account(Some(siwc));
        control.set_account(None);
        assert!(control.app.chatgpt_ready);
        assert_eq!(control.app.account_label, "native@example.invalid");
        assert!(control.app.account_usage.snapshot.is_some());
    }

    #[test]
    fn native_disconnect_preserves_siwc_fallback_and_antigravity_startup() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        control.set_account(Some(auth::AccountInfo {
            email: Some("siwc@example.invalid".into()),
            subject: "siwc-user".into(),
            plan_usage: true,
        }));
        control.set_native_account(Some(native_account("native@example.invalid")));
        control.action(DesktopAction::OpenAntigravity);
        let agy = control.app.active_panel().id;
        let agy_generation = control.codex[&agy].generation;
        control.action(DesktopAction::OpenCodex {
            model: "gpt-6.1-sol".into(),
        });
        let codex = control.app.active_panel().id;
        assert!(control.codex.contains_key(&codex));
        control.action(DesktopAction::DisconnectCodexAccount);
        assert!(!control.codex.contains_key(&codex));
        assert_eq!(control.codex[&agy].generation, agy_generation);
        assert!(!*control.codex[&agy].cancel.borrow());
        assert!(control.app.native_codex_account.is_none());
        assert!(control.app.chatgpt_ready);
        assert_eq!(control.app.account_label, "siwc@example.invalid");
        assert!(control.native_job.as_ref().unwrap().logout);
    }

    #[test]
    fn stale_native_login_and_install_progress_cannot_mutate_current_state() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.native_generation = 8;
        control.set_native_account(Some(native_account("current@example.invalid")));
        control
            .sender
            .send(Event::NativeLogin(
                7,
                codex_account::LoginEvent::Completed(native_account("stale@example.invalid")),
            ))
            .unwrap();
        let id = control.app.active_panel().id;
        control
            .sender
            .send(Event::TerminalProgress(id, 42, "stale".into()))
            .unwrap();
        control.poll();
        assert_eq!(control.app.account_label, "current@example.invalid");
        assert!(control.app.terminal_notices.is_empty());
        assert!(control.app.native_codex_login_url.is_none());
    }

    fn controller(project: &Path, runtime: &tokio::runtime::Runtime) -> DesktopController {
        DesktopController::new(
            project.into(),
            true,
            true,
            egui::Context::default(),
            runtime.handle().clone(),
        )
        .unwrap()
    }

    #[test]
    fn removing_active_workspace_saves_stops_work_and_reopen_restores_list_entry() {
        let directory = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.save_path = Some(data.path().join("session.json"));
        control.workspace_preferences_path = Some(data.path().join("workspaces.json"));
        let project = control.app.project_path.clone();
        let id = control.app.active_panel().id;
        control.app.active_panel_mut().input = "running prompt".into();
        control.action(DesktopAction::Send(id));
        control.app.active_panel_mut().input = "preserved draft".into();
        let old_generation = control.workers[&id].generation;
        let (cancel, watch) = tokio::sync::watch::channel(false);
        control.codex.insert(
            id,
            CodexPanel {
                generation: 7,
                cancel,
                task: runtime.spawn(std::future::pending()),
            },
        );
        control.action(DesktopAction::SetProjectPinned {
            project: project.clone(),
            pinned: true,
        });
        assert!(
            control
                .app
                .recent_projects
                .iter()
                .find(|entry| entry.path == project)
                .unwrap()
                .pinned
        );
        control.action(DesktopAction::RemoveProject(project.clone()));
        assert!(control.workers.is_empty());
        assert!(control.codex.is_empty());
        assert!(*watch.borrow());
        assert!(control.terminals.is_empty());
        assert_eq!(control.app.project_path, project);
        assert!(
            !control
                .app
                .recent_projects
                .iter()
                .any(|entry| entry.path == project)
        );
        assert_eq!(control.app.active_panel().input, "preserved draft");
        control.provider_event(
            old_generation,
            ProviderEvent::Delta {
                panel_id: id,
                text: "stale".into(),
            },
        );
        assert_eq!(control.app.active_panel().status, PanelStatus::Cancelled);
        let saved = storage::load(control.save_path.as_ref().unwrap(), &project)
            .unwrap()
            .unwrap();
        assert_eq!(saved.panels[0].input, "preserved draft");
        let mut restarted = controller(directory.path(), &runtime);
        restarted.workspace_preferences_path = control.workspace_preferences_path.clone();
        restarted.workspace_preferences = storage::load_workspace_preferences(
            restarted.workspace_preferences_path.as_ref().unwrap(),
        )
        .unwrap();
        restarted.refresh_recent_projects();
        assert!(
            !restarted
                .app
                .recent_projects
                .iter()
                .any(|entry| entry.path == project)
        );
        restarted.switch_project(project.clone()).unwrap();
        assert!(
            restarted
                .app
                .recent_projects
                .iter()
                .any(|entry| entry.path == project && !entry.pinned)
        );
        assert!(
            !storage::load_workspace_preferences(
                restarted.workspace_preferences_path.as_ref().unwrap()
            )
            .unwrap()
            .is_hidden(&project)
        );
    }

    #[test]
    fn failed_workspace_removal_preserves_list_and_preference_file() {
        let directory = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let project = control.app.project_path.clone();
        let path = data.path().join("session.json");
        std::fs::write(&path, b"{invalid").unwrap();
        control.save_path = Some(path.clone());
        control.workspace_preferences_path = Some(data.path().join("workspaces.json"));
        control.action(DesktopAction::RemoveProject(project.clone()));
        assert!(
            control
                .app
                .recent_projects
                .iter()
                .any(|entry| entry.path == project)
        );
        assert!(!control.workspace_preferences.is_hidden(&project));
        assert!(
            !control
                .workspace_preferences_path
                .as_ref()
                .unwrap()
                .exists()
        );
        assert_eq!(std::fs::read(&path).unwrap(), b"{invalid");
        assert!(control.app.notice.contains("Saving the session failed"));
    }

    #[test]
    fn removing_inactive_workspace_keeps_current_worker_running() {
        let directory = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let project = control.app.project_path.clone();
        let other_path = other.path().canonicalize().unwrap();
        let id = control.app.active_panel().id;
        control.app.active_panel_mut().input = "continue".into();
        control.action(DesktopAction::Send(id));
        control.action(DesktopAction::SetProjectPinned {
            project: other_path.clone(),
            pinned: true,
        });
        assert_eq!(control.app.recent_projects[0].path, other_path);
        control.action(DesktopAction::RemoveProject(other_path.clone()));
        assert!(control.workers.contains_key(&id));
        assert_eq!(control.app.active_panel().status, PanelStatus::Running);
        assert_eq!(control.app.project_path, project);
        control.switch_project(other_path.clone()).unwrap();
        assert!(!control.workspace_preferences.is_hidden(&other_path));
        assert!(
            control
                .app
                .recent_projects
                .iter()
                .any(|entry| entry.path == other_path)
        );
    }

    #[test]
    fn launcher_selects_the_matching_provider_and_account_model() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.available_models = vec![ModelChoice {
            id: "account-model".into(),
            name: "Account model".into(),
        }];
        control.action(DesktopAction::OpenCodex {
            model: String::new(),
        });
        assert_eq!(control.app.active_panel().provider, ProviderKind::Chatgpt);
        assert_eq!(control.app.active_panel().model, "account-model");
        let first = control.app.active_panel().id;
        control.action(DesktopAction::OpenAgent {
            title: "Claude".into(),
            model: "anthropic/claude-sonnet-4".into(),
        });
        assert_eq!(
            control.app.active_panel().provider,
            ProviderKind::OpenRouter
        );
        assert_eq!(
            control.app.active_panel().model,
            "anthropic/claude-sonnet-4"
        );
        assert_ne!(control.app.active_panel().id, first);
        assert_eq!(
            control
                .app
                .panels
                .iter()
                .find(|panel| panel.id == first)
                .unwrap()
                .model,
            "account-model"
        );
    }

    #[test]
    fn antigravity_launch_does_not_require_chatgpt_or_openrouter_credentials() {
        let directory = tempfile::tempdir().unwrap();
        // Leave the runtime idle so the real CLI and account services never run.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        control.openrouter_key.clear();
        assert!(!control.app.chatgpt_ready);
        control.action(DesktopAction::OpenAntigravity);
        let panel = control.app.active_panel();
        assert_eq!(panel.provider, ProviderKind::Antigravity);
        assert_eq!(panel.title, "Charlotte");
        assert_eq!(panel.model, "antigravity/default");
        assert!(control.terminal_starting(panel.id));
        assert!(control.login.is_none());
        assert!(control.account_job.is_none());
        assert!(control.workers.is_empty());
        assert!(!control.app.chatgpt_ready);
    }

    #[test]
    fn chatgpt_login_and_logout_preserve_antigravity_startup() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        control.action(DesktopAction::OpenAntigravity);
        let id = control.app.active_panel().id;
        let generation = control.codex[&id].generation;
        control.login_event(LoginEvent::Completed(auth::AccountInfo {
            email: None,
            subject: "test-user".into(),
            plan_usage: true,
        }));
        assert_eq!(
            control.app.active_panel().provider,
            ProviderKind::Antigravity
        );
        assert_eq!(control.codex[&id].generation, generation);
        // Complete the queued model job without running external account operations.
        control.account_event(AccountResult::Models(Ok(Vec::new())));
        control.action(DesktopAction::Logout);
        assert_eq!(control.codex[&id].generation, generation);
        assert!(!*control.codex[&id].cancel.borrow());
        assert!(!control.app.chatgpt_ready);
        assert_eq!(
            control.app.active_panel().provider,
            ProviderKind::Antigravity
        );
    }

    #[test]
    fn closing_panel_cancels_pending_terminal_startup_without_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let id = control.app.panels[0].id;
        control.app.panels[0].provider = ProviderKind::Antigravity;
        let (cancel, mut cancelled) = tokio::sync::watch::channel(false);
        let task = runtime.spawn(async move {
            let _ = cancelled.changed().await;
            assert!(*cancelled.borrow());
        });
        control.codex.insert(
            id,
            CodexPanel {
                generation: 17,
                cancel,
                task,
            },
        );
        control.action(DesktopAction::ClosePanel(id));
        assert!(!control.codex.contains_key(&id));
        assert!(!control.app.panels.iter().any(|panel| panel.id == id));
        let tasks = std::mem::take(&mut control.retired);
        runtime.block_on(async {
            for task in tasks {
                task.await.unwrap();
            }
        });
        // A late failure from the retired launch cannot affect a different panel.
        let notice = control.app.notice.clone();
        control.codex_status(id, 17, Err("stale terminal failure".into()));
        assert_eq!(control.app.notice, notice);
    }

    #[test]
    fn restart_restores_chats_and_two_native_panels_without_starting_any_process() {
        let project = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let mut app = App::new(project.path().into(), true);
        for (index, provider) in [ProviderKind::Chatgpt, ProviderKind::Antigravity]
            .into_iter()
            .enumerate()
        {
            let key = uuid::Uuid::new_v4().to_string();
            app.panels[index].provider = provider;
            app.panels[index].terminal_key = Some(key.clone());
            let store = TerminalStore::at(data.path().into(), &key, provider).unwrap();
            let mut parser = vt100::Parser::new(8, 50, 1000);
            parser.process(format!("Saved output {index}\r\n› unfinished draft").as_bytes());
            store.save(parser.screen()).unwrap();
        }
        app.panels[2].messages.push(Message {
            role: Role::User,
            content: "saved chat history".into(),
        });
        app.panels[2].input = "saved chat draft".into();
        let path = data.path().join("workspace.json");
        storage::save(&path, &app).unwrap();
        let session = storage::load(&path, project.path()).unwrap().unwrap();
        app.panels = session.panels;
        let mut terminals = restore_saved_terminals_with(
            &mut app,
            &egui::Context::default(),
            false,
            |key, provider| {
                Ok(Arc::new(TerminalStore::at(
                    data.path().into(),
                    key,
                    provider,
                )?))
            },
        )
        .unwrap();
        assert_eq!(terminals.len(), 2);
        for index in 0..2 {
            let terminal = terminals.get_mut(&app.panels[index].id).unwrap();
            assert!(terminal.is_restored());
            assert!(
                terminal
                    .snapshot()
                    .contents()
                    .contains(&format!("Saved output {index}"))
            );
            assert!(terminal.write(b"must not send").is_err());
            assert!(app.panels[index].terminal_saved);
        }
        assert_eq!(app.panels[2].input, "saved chat draft");
        assert_eq!(
            app.panels[2].messages.last().unwrap().content,
            "saved chat history"
        );
        assert!(
            restore_saved_terminals_with(&mut app, &egui::Context::default(), true, |_, _| panic!(
                "no-save must not read terminal archives"
            ))
            .unwrap()
            .is_empty()
        );
    }

    #[test]
    fn last_native_panel_closes_cancels_startup_and_allows_a_new_panel() {
        for provider in [ProviderKind::Chatgpt, ProviderKind::Antigravity] {
            let directory = tempfile::tempdir().unwrap();
            let data = tempfile::tempdir().unwrap();
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let mut control = controller(directory.path(), &runtime);
            control.app.panels.truncate(1);
            control.app.active = 0;
            control.app.panels[0].provider = provider;
            let id = control.app.panels[0].id;
            let (cancel, cancelled) = tokio::sync::watch::channel(false);
            control.codex.insert(
                id,
                CodexPanel {
                    generation: 9,
                    cancel,
                    task: runtime.spawn(std::future::pending()),
                },
            );
            control.app.terminal_notices.insert(id, "Starting".into());
            control.action(DesktopAction::ClosePanel(id));
            assert!(control.app.panels.is_empty());
            assert!(control.codex.is_empty());
            assert!(control.app.terminal_notices.is_empty());
            assert!(*cancelled.borrow());
            let path = data.path().join("session.json");
            storage::save(&path, &control.app).unwrap();
            let restored = storage::load(&path, directory.path()).unwrap().unwrap();
            assert!(restored.panels.is_empty());
            assert_eq!(restored.active, 0);
            control.codex_status(id, 9, Err("late result".into()));
            control.poll();
            control.action(DesktopAction::ClosePanel(id));
            assert!(control.app.panels.is_empty());
            control.action(DesktopAction::NewPanel);
            assert_eq!(control.app.panels.len(), 1);
            assert_ne!(control.app.active_panel().id, id);
        }
    }

    #[test]
    fn terminal_startup_generation_rejects_old_result_for_same_panel() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.active_panel_mut().provider = ProviderKind::Antigravity;
        let id = control.app.active_panel().id;
        let (cancel, _) = tokio::sync::watch::channel(false);
        control.codex.insert(
            id,
            CodexPanel {
                generation: 42,
                cancel,
                task: runtime.spawn(async {}),
            },
        );
        let notice = control.app.notice.clone();
        control.codex_status(id, 41, Err("stale terminal failure".into()));
        assert_eq!(control.app.notice, notice);
        assert!(control.terminal_starting(id));
        control.codex_status(id, 42, Err("current terminal failure".into()));
        assert!(!control.terminal_starting(id));
        assert_eq!(control.app.active_panel().status, PanelStatus::Error);
        assert!(control.app.notice.contains("current terminal failure"));
    }

    #[test]
    fn codex_launch_requires_vibe_login_and_custom_titles_stay_openrouter() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        let count = control.app.panels.len();
        control.action(DesktopAction::OpenCodex {
            model: String::new(),
        });
        assert_eq!(control.app.panels.len(), count);
        assert!(control.codex.is_empty());
        assert!(control.workers.is_empty());
        assert!(control.app.notice.contains("jelentkezz be"));
        control.action(DesktopAction::OpenAgent {
            title: "Codex CLI".into(),
            model: "custom/provider-model".into(),
        });
        assert_eq!(
            control.app.active_panel().provider,
            ProviderKind::OpenRouter
        );
        assert_eq!(control.app.active_panel().model, "custom/provider-model");
        assert!(control.codex.is_empty());
    }

    #[test]
    fn openrouter_key_is_session_only_and_invalid_keys_preserve_previous_value() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let fingerprint_before = fingerprint(&control.app);
        control.action(DesktopAction::SetOpenRouterKey(
            "  mock-private-key  ".into(),
        ));
        assert_eq!(control.openrouter_key, "mock-private-key");
        assert_eq!(fingerprint(&control.app), fingerprint_before);
        assert!(!control.app.notice.contains("mock-private-key"));
        for key in [String::new(), "has\ncontrol".into(), "x".repeat(8193)] {
            control.action(DesktopAction::SetOpenRouterKey(key));
            assert_eq!(control.openrouter_key, "mock-private-key");
        }
    }

    #[test]
    fn file_attachments_stay_panel_scoped_and_do_not_start_inference() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let first = control.app.panels[0].id;
        let second = control.app.panels[1].id;
        let before = fingerprint(&control.app);
        assert!(control.file_jobs.is_empty());
        control.app.files.preview = Some((
            crate::project_files::Request::File("a.rs".into()),
            crate::project_files::Load::Ready("local file".into()),
        ));
        control.action(DesktopAction::AttachFile {
            panel_id: first,
            path: "a.rs".into(),
        });
        assert_eq!(control.app.panels[0].attachments, vec!["a.rs"]);
        assert!(control.app.panels[1].attachments.is_empty());
        assert_ne!(before, fingerprint(&control.app));
        assert!(control.workers.is_empty());
        assert!(control.app.files.preview.is_none());
        control.action(DesktopAction::RemoveAttachment {
            panel_id: second,
            path: "a.rs".into(),
        });
        assert_eq!(control.app.panels[0].attachments.len(), 1);
        control.action(DesktopAction::RemoveAttachment {
            panel_id: first,
            path: "a.rs".into(),
        });
        assert!(control.app.panels[0].attachments.is_empty());
    }

    #[test]
    fn prepared_attachment_snapshots_are_saved_only_for_the_current_worker() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let id = control.app.panels[0].id;
        control.app.panels[0].messages.push(Message {
            role: Role::User,
            content: "prompt".into(),
        });
        control.app.panels[0].attachments = vec!["a.rs".into()];
        control.workers.insert(
            id,
            Worker {
                generation: 42,
                task: runtime.spawn(std::future::pending()),
            },
        );
        control.provider_event(
            41,
            ProviderEvent::PreparedInput {
                panel_id: id,
                content: "stale".into(),
            },
        );
        assert_eq!(control.app.panels[0].attachments.len(), 1);
        control.provider_event(
            42,
            ProviderEvent::PreparedInput {
                panel_id: id,
                content: "snapshot".into(),
            },
        );
        assert!(control.app.panels[0].attachments.is_empty());
        assert_eq!(
            control.app.panels[0].messages.last().unwrap().content,
            "snapshot"
        );
    }

    #[test]
    fn openrouter_review_is_panel_scoped_cancelled_and_never_restored_as_permission() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let id = control.app.panels[0].id;
        control.app.panels[0].status = PanelStatus::Running;
        control.workers.insert(
            id,
            Worker {
                generation: 42,
                task: runtime.spawn(std::future::pending()),
            },
        );
        let edit = crate::workspace_tools::EditProposal {
            path: "test.rs".into(),
            before: None,
            after: "hello".into(),
        };
        let (reply, mut decision) = tokio::sync::oneshot::channel();
        control.provider_event(
            41,
            ProviderEvent::Review {
                panel_id: id,
                edit: edit.clone(),
                reply,
            },
        );
        assert!(control.app.edit_reviews.is_empty());
        assert!(decision.try_recv().is_err());
        let (reply, mut decision) = tokio::sync::oneshot::channel();
        control.provider_event(
            42,
            ProviderEvent::Review {
                panel_id: id,
                edit,
                reply,
            },
        );
        assert_eq!(control.app.edit_reviews.len(), 1);
        let save = directory.path().join("session.json");
        storage::save(&save, &control.app).unwrap();
        assert!(storage::load(&save, directory.path()).unwrap().is_some());
        assert!(
            !std::fs::read_to_string(&save)
                .unwrap()
                .contains("edit_reviews")
        );
        control.action(DesktopAction::ReviewEdit {
            panel_id: id + 100,
            approve: true,
        });
        assert_eq!(control.app.edit_reviews.len(), 1);
        assert!(decision.try_recv().is_err());
        control.action(DesktopAction::Cancel(id));
        assert!(control.app.edit_reviews.is_empty());
        assert!(control.edit_replies.is_empty());
        assert!(decision.try_recv().is_err());
        assert!(!directory.path().join("test.rs").exists());
    }

    #[test]
    fn openrouter_slash_commands_work_without_network_or_api_key() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.demo = false;
        control.openrouter_key.clear();
        let id = control.app.panels[0].id;
        for command in ["/help", "/model test/model", "/model"] {
            control.app.panels[0].input = command.into();
            control.action(DesktopAction::Send(id));
            assert!(control.workers.is_empty());
            assert!(control.app.panels[0].input.is_empty());
        }
        assert_eq!(control.app.panels[0].model, "test/model");
        control.app.panels[0].input = "/clear".into();
        control.action(DesktopAction::Send(id));
        assert!(control.app.panels[0].messages.is_empty());
    }

    #[test]
    fn demo_send_cancel_and_stale_events_are_independent() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let first = control.app.panels[0].id;
        let second = control.app.panels[1].id;
        control.app.panels[0].input = "first\u{1b}\r\u{0}\n\tline".into();
        control.app.panels[1].input = "second".into();
        control.action(DesktopAction::Send(first));
        control.action(DesktopAction::Send(second));
        assert_eq!(
            control.app.panels[0]
                .messages
                .iter()
                .rev()
                .find(|message| message.role == Role::User)
                .unwrap()
                .content,
            "first\n\tline"
        );
        let generation = control.workers[&first].generation;
        control.action(DesktopAction::Cancel(first));
        control.provider_event(
            generation,
            ProviderEvent::Delta {
                panel_id: first,
                text: "stale".into(),
            },
        );
        assert_eq!(control.app.panels[0].status, PanelStatus::Cancelled);
        assert_eq!(control.app.panels[1].status, PanelStatus::Running);
        assert!(
            !control.app.panels[0]
                .messages
                .iter()
                .any(|message| message.content.contains("stale"))
        );
        assert!(control.workers.contains_key(&second));
    }

    #[test]
    fn closing_one_panel_keeps_other_worker_and_new_panel_inherits_source() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.panels[0].provider = ProviderKind::Chatgpt;
        control.app.panels[0].model = "chosen-model".into();
        control.action(DesktopAction::NewPanel);
        let new_id = control.app.active_panel().id;
        assert_eq!(control.app.active_panel().provider, ProviderKind::Chatgpt);
        assert_eq!(control.app.active_panel().model, "chosen-model");
        control.app.active_panel_mut().input = "demo".into();
        control.action(DesktopAction::Send(new_id));
        let first = control.app.panels[0].id;
        control.action(DesktopAction::ClosePanel(first));
        assert!(control.workers.contains_key(&new_id));
        assert!(!control.app.panels.iter().any(|panel| panel.id == first));
    }

    #[test]
    fn switching_workspace_persists_selection_and_failed_switch_keeps_it() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(first.path(), &runtime);
        let path = data.path().join("workspaces.json");
        control.workspace_preferences_path = Some(path.clone());
        control.switch_project(second.path().into()).unwrap();
        let selected = second.path().canonicalize().unwrap();
        assert_eq!(
            storage::load_workspace_preferences(&path)
                .unwrap()
                .startup_project(),
            Some(selected.clone())
        );
        assert!(control.switch_project(data.path().join("missing")).is_err());
        assert_eq!(
            storage::load_workspace_preferences(&path)
                .unwrap()
                .startup_project(),
            Some(selected)
        );
        control.switch_project(first.path().into()).unwrap();
        assert_eq!(
            storage::load_workspace_preferences(&path)
                .unwrap()
                .startup_project(),
            Some(first.path().canonicalize().unwrap())
        );
    }

    #[test]
    fn failed_project_switch_preserves_current_app_and_workers() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.app.active_panel_mut().input = "draft".into();
        let old = control.app.project_path.clone();
        assert!(
            control
                .switch_project(directory.path().join("missing"))
                .is_err()
        );
        assert_eq!(control.app.project_path, old);
        assert_eq!(control.app.active_panel().input, "draft");
    }

    #[test]
    fn switching_projects_rejects_old_generation_even_when_panel_ids_repeat() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(first.path(), &runtime);
        let id = control.app.active_panel().id;
        control.app.active_panel_mut().input = "old".into();
        control.action(DesktopAction::Send(id));
        let old_generation = control.workers[&id].generation;
        control.app.account_label = "preserved-account-label".into();
        control.switch_project(second.path().into()).unwrap();
        assert_eq!(control.app.account_label, "preserved-account-label");
        control.app.active_panel_mut().input = "new".into();
        control.action(DesktopAction::Send(id));
        control.provider_event(old_generation, ProviderEvent::Finished { panel_id: id });
        assert_eq!(control.app.active_panel().status, PanelStatus::Running);
    }

    #[test]
    fn close_is_nonblocking_and_waits_for_worker_shutdown() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let id = control.app.active_panel().id;
        control.app.active_panel_mut().input = "demo".into();
        control.action(DesktopAction::Send(id));
        control.request_close();
        assert!(control.closing());
        let deadline = Instant::now() + Duration::from_secs(3);
        while !control.ready_to_close() && Instant::now() < deadline {
            control.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(control.ready_to_close());
        assert!(control.workers.is_empty());
        assert!(control.save_path.is_none());
        assert!(control.project_lock.is_none());
    }

    #[test]
    fn fingerprint_detects_direct_gui_drafts_without_auth_or_network() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let before = fingerprint(&control.app);
        control.app.active_panel_mut().input = "direct egui edit".into();
        assert_ne!(fingerprint(&control.app), before);
        let before_effort = fingerprint(&control.app);
        let panel_id = control.app.active_panel().id;
        control.action(DesktopAction::SetEffort {
            panel_id,
            effort: crate::app::Effort::Deep,
        });
        assert_ne!(fingerprint(&control.app), before_effort);
        control.action(DesktopAction::Login);
        control.action(DesktopAction::Logout);
        control.action(DesktopAction::RefreshModels);
        assert!(control.login.is_none());
        assert!(control.account_job.is_none());
        assert!(!control.app.auth_busy);
        assert!(!control.app.notice.contains("F1:"));
    }

    #[test]
    fn missing_chatgpt_login_preserves_draft_and_never_starts_worker() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        // Toggle only after demo construction so this test never loads account credentials.
        control.app.demo = false;
        control.app.active_panel_mut().provider = ProviderKind::Chatgpt;
        control.app.active_panel_mut().input = "keep this draft".into();
        let id = control.app.active_panel().id;
        let messages = control.app.active_panel().messages.len();
        control.action(DesktopAction::Send(id));
        assert_eq!(control.app.active_panel().input, "keep this draft");
        assert_eq!(control.app.active_panel().messages.len(), messages);
        assert!(control.workers.is_empty());
        assert!(control.account_job.is_none());
    }

    #[test]
    fn stale_account_events_cannot_replace_current_identity() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        control.account_generation = 9;
        control.app.account_label = "current".into();
        assert!(
            control
                .sender
                .send(Event::Account(8, AccountResult::Status(Ok(None))))
                .is_ok()
        );
        assert!(
            control
                .sender
                .send(Event::Login(
                    7,
                    LoginEvent::Waiting {
                        authorization_url: "stale".into(),
                    },
                ))
                .is_ok()
        );
        control.poll();
        assert_eq!(control.app.account_label, "current");
        assert!(control.app.login_url.is_none());
        assert!(control.account_job.is_none());
    }

    #[test]
    fn failed_final_save_keeps_window_open_and_draft_available() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        let path = directory.path().join("corrupt-session.json");
        std::fs::write(&path, "invalid session").unwrap();
        control.save_path = Some(path.clone());
        control.app.active_panel_mut().input = "unsaved draft".into();
        control.closing = true;
        assert!(control.sender.send(Event::Shutdown).is_ok());
        control.poll();
        assert!(!control.closing());
        assert!(!control.ready_to_close());
        assert_eq!(control.app.active_panel().input, "unsaved draft");
        assert_eq!(std::fs::read_to_string(path).unwrap(), "invalid session");
        assert!(
            control.app.notice.contains("Ment")
                || control.app.notice.contains("Saving the session failed")
        );
    }

    #[test]
    fn completed_login_selects_chatgpt_only_when_plan_usage_is_granted() {
        let directory = tempfile::tempdir().unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let mut control = controller(directory.path(), &runtime);
        assert_eq!(
            control.app.active_panel().provider,
            ProviderKind::OpenRouter
        );
        control.login_event(LoginEvent::Completed(auth::AccountInfo {
            email: Some("test@example.invalid".into()),
            subject: "test-user".into(),
            plan_usage: false,
        }));
        assert_eq!(
            control.app.active_panel().provider,
            ProviderKind::OpenRouter
        );
        assert!(!control.app.chatgpt_ready);
        control.login_event(LoginEvent::Completed(auth::AccountInfo {
            email: Some("test@example.invalid".into()),
            subject: "test-user".into(),
            plan_usage: true,
        }));
        assert_eq!(control.app.active_panel().provider, ProviderKind::Chatgpt);
        assert!(control.app.chatgpt_ready);
        assert!(control.account_job.is_none());
    }
}
