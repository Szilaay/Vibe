#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use clap::Parser;
use eframe::egui;
use vibe::{
    desktop_controller::DesktopController,
    desktop_types::DesktopAction,
    desktop_ui::{self, DesktopView},
    storage, terminal_widget,
};

#[derive(Parser)]
#[command(version, about = "VIBE - native multi-agent AI workspace")]
struct Options {
    #[arg(short, long)]
    project: Option<PathBuf>,
    #[arg(long)]
    demo: bool,
    #[arg(long)]
    no_save: bool,
    #[arg(long)]
    login: bool,
}

fn main() {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--record-cli-session")
    {
        // Native CLI lifecycle hook: no GUI, network, or model context.
        if let Err(error) = vibe::terminal_persistence::record_hook() {
            eprintln!("VIBE session metadata: {error}");
        }
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), "{{}}");
        return;
    }
    if let Err(message) = launch() {
        rfd::MessageDialog::new()
            .set_title("VIBE - startup error")
            .set_description(message)
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}

fn launch() -> Result<(), String> {
    let options = Options::try_parse().map_err(|error| error.to_string())?;
    let mut project = options.project.clone();
    let mut startup_error = String::new();
    let mut startup_projects = Vec::new();
    if options.demo && project.is_none() {
        project = std::env::current_dir().ok();
    } else if !options.demo && !options.no_save {
        match storage::workspace_preferences_path()
            .and_then(|path| storage::load_workspace_preferences(&path))
        {
            Ok(preferences) => {
                startup_projects = storage::recent_projects(20, &preferences);
            }
            Err(error) => startup_error = error.to_string(),
        }
    }
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|_| "Cannot start the background runtime.".to_owned())?;
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("VIBE")
            .with_decorations(false)
            .with_inner_size([1500.0, 960.0])
            .with_min_inner_size([1000.0, 700.0])
            .with_icon(icon()),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "VIBE",
        native,
        Box::new(move |cc| {
            desktop_ui::configure(&cc.egui_ctx);
            let mut app = NativeApp {
                controller: None,
                view: DesktopView::launcher(),
                runtime,
                project,
                demo: options.demo,
                no_save: options.no_save,
                auto_login: options.login,
                startup_error,
                welcome: {
                    let mut welcome = vibe::app::App::welcome();
                    welcome.recent_projects = startup_projects;
                    welcome
                },
                welcome_key: None,
                pending_cli: None,
                pending_login: false,
                voice: vibe::voice::VoiceInput::default(),
                updates: Default::default(),
            };
            app.initialize(&cc.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| format!("Cannot start the graphical interface: {error}"))
}

struct NativeApp {
    updates: vibe::update_ui::UpdateCenter,
    voice: vibe::voice::VoiceInput,
    controller: Option<DesktopController>,
    view: DesktopView,
    runtime: tokio::runtime::Runtime,
    project: Option<PathBuf>,
    demo: bool,
    no_save: bool,
    auto_login: bool,
    startup_error: String,
    welcome: vibe::app::App,
    welcome_key: Option<String>,
    pending_cli: Option<DesktopAction>,
    pending_login: bool,
}

impl NativeApp {
    fn initialize(&mut self, ctx: &egui::Context) {
        let Some(project) = self.project.clone() else {
            return;
        };
        match DesktopController::new(
            project,
            self.demo,
            self.no_save,
            ctx.clone(),
            self.runtime.handle().clone(),
        ) {
            Ok(mut controller) => {
                if let Some(key) = self.welcome_key.take() {
                    controller.action(DesktopAction::SetOpenRouterKey(key));
                }
                if controller.app.panels.iter().any(|panel| {
                    panel.terminal_key.is_some()
                        || !panel.input.is_empty()
                        || panel
                            .messages
                            .iter()
                            .any(|message| message.role == vibe::app::Role::User)
                }) {
                    self.view.launcher = false;
                }
                self.controller = Some(controller);
                self.startup_error.clear();
            }
            Err(error) => self.startup_error = error.to_string(),
        }
    }

    fn startup(&mut self, ctx: &egui::Context) {
        let actions = desktop_ui::draw(ctx, &mut self.welcome, &mut self.view, false);
        if !self.startup_error.is_empty() {
            let mut open = true;
            let mut without_save = false;
            egui::Window::new("Munkaterület megnyitása")
                .resizable(false)
                .open(&mut open)
                .show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(&self.startup_error);
                    if self.project.is_some() && ui.button("Megnyitás mentés nélkül").clicked()
                    {
                        without_save = true;
                    }
                });
            if !open {
                self.startup_error.clear();
            }
            if without_save {
                self.no_save = true;
                self.initialize(ctx);
            }
        }
        for action in actions {
            match action {
                DesktopAction::CheckUpdates => self.updates.request_check(ctx, !self.demo),
                DesktopAction::SetOpenRouterKey(key) => self.welcome_key = Some(key),
                DesktopAction::NewPanel => self.view.launcher = true,
                DesktopAction::RemoveProject(ref project)
                | DesktopAction::SetProjectPinned { ref project, .. } => {
                    if !self.no_save && !self.demo {
                        let result = (|| -> anyhow::Result<()> {
                            let path = storage::workspace_preferences_path()?;
                            let mut preferences = storage::load_workspace_preferences(&path)?;
                            match &action {
                                DesktopAction::RemoveProject(_) => preferences.remove(project),
                                DesktopAction::SetProjectPinned { pinned, .. } => {
                                    preferences.set_pinned(project, *pinned)
                                }
                                _ => (),
                            }
                            storage::save_workspace_preferences(&path, &preferences)?;
                            self.welcome.recent_projects =
                                storage::recent_projects(20, &preferences);
                            Ok(())
                        })();
                        if let Err(error) = result {
                            self.startup_error = error.to_string();
                        }
                    }
                }
                DesktopAction::SwitchProject(ref project)
                | DesktopAction::SelectProjectPanel { ref project, .. } => {
                    self.project = Some(project.clone());
                    self.initialize(ctx);
                    if let Some(controller) = self.controller.as_mut() {
                        controller.action(action);
                    }
                }
                DesktopAction::Browse(_)
                | DesktopAction::RefreshFiles
                | DesktopAction::CloseFilePreview => {
                    self.welcome.notice = "Előbb válassz CLI-t és projektmappát.".into();
                }
                other @ (DesktopAction::OpenProject
                | DesktopAction::OpenCodex { .. }
                | DesktopAction::OpenAntigravity
                | DesktopAction::OpenAgent { .. }
                | DesktopAction::Login
                | DesktopAction::ConnectCodexAccount) => {
                    // Only an explicit selection can create a workspace. Never use process CWD.
                    if let Some(project) = pick_project() {
                        self.project = Some(project);
                        self.initialize(ctx);
                        if let Some(controller) = self.controller.as_mut() {
                            if let Some(key) = self.welcome_key.take() {
                                controller.action(DesktopAction::SetOpenRouterKey(key));
                            }
                            if !matches!(other, DesktopAction::OpenProject) {
                                self.pending_cli = Some(other);
                                self.pending_login = false;
                            }
                        }
                    }
                    self.view.launcher = true;
                }
                _ => (),
            }
        }
    }
}

impl eframe::App for NativeApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(plan) = self.updates.show(ctx, !self.demo) {
            match plan.launch() {
                Ok(()) => {
                    if let Some(controller) = self.controller.as_mut() {
                        controller.request_close();
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                Err(error) => self
                    .updates
                    .failure(format!("A telepítő nem indítható: {error}")),
            }
        }
        let Some(controller) = self.controller.as_mut() else {
            self.startup(ctx);
            return;
        };
        controller.poll();
        if self.pending_cli.is_some()
            && !controller.app.native_codex_busy
            && !controller.app.auth_busy
        {
            let needs_login = matches!(self.pending_cli, Some(DesktopAction::OpenCodex { .. }))
                && !controller.app.chatgpt_ready;
            if needs_login && !self.pending_login {
                self.pending_login = true;
                controller.action(DesktopAction::ConnectCodexAccount);
            } else if needs_login {
                self.pending_cli = None;
                self.view.launcher = true;
            } else if let Some(action) = self.pending_cli.take() {
                controller.action(action);
                self.view.launcher = controller.app.panels.is_empty();
            }
        }
        if self.auto_login && !controller.app.auth_busy {
            self.auto_login = false;
            controller.action(DesktopAction::Login);
        }
        if ctx.input(|input| input.viewport().close_requested()) && !controller.ready_to_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            controller.request_close();
        }
        let closing = controller.closing();
        self.voice.poll();
        let dictation_allowed = !closing
            && !egui::Popup::is_any_open(ctx)
            && self.view.terminal_input_allowed(&controller.app)
            && controller
                .app
                .panels
                .get(controller.app.active)
                .is_some_and(|panel| controller.terminals.contains_key(&panel.id));
        if closing {
            self.voice.cancel();
        }
        if self.voice.show(ctx, dictation_allowed) {
            let panel = controller.app.active_panel();
            let target = controller
                .terminals
                .get(&panel.id)
                .ok_or_else(|| anyhow::anyhow!("Válassz egy futó CLI-ablakot."))
                .and_then(|terminal| terminal.dictation_target());
            self.voice.start(target, panel.title.clone(), ctx);
        }
        let mut actions = desktop_ui::draw(ctx, &mut controller.app, &mut self.view, closing);
        let terminal_input =
            self.view.terminal_input_allowed(&controller.app) && !closing && !self.voice.learning();
        if terminal_input {
            for (id, rect) in self.view.terminal_regions() {
                if let Some(terminal) = controller.terminals.get_mut(&id) {
                    if terminal_widget::show(
                        ctx,
                        id,
                        rect,
                        terminal,
                        controller.app.active_panel().id == id,
                    ) {
                        controller.app.focus_id(id);
                    }
                } else {
                    let antigravity = controller.app.panels.iter().any(|panel| {
                        panel.id == id && panel.provider == vibe::app::ProviderKind::Antigravity
                    });
                    let cli_name = if antigravity {
                        "Antigravity CLI"
                    } else {
                        "Codex CLI"
                    };
                    egui::Area::new(egui::Id::new(("terminal_placeholder", id)))
                        .order(egui::Order::Middle)
                        .fixed_pos(rect.min)
                        .show(ctx, |ui| {
                            ui.set_clip_rect(rect);
                            ui.set_min_size(rect.size());
                            ui.set_max_size(rect.size());
                            ui.vertical_centered(|ui| {
                                ui.add_space((rect.height() * 0.3).max(0.0));
                                ui.label(format!("Original {cli_name} terminal"));
                                ui.label(
                                    egui::RichText::new(
                                        controller
                                            .app
                                            .terminal_notices
                                            .get(&id)
                                            .unwrap_or(&controller.app.notice),
                                    )
                                    .small(),
                                );
                                if controller.terminal_starting(id) {
                                    ui.spinner();
                                } else if ui.button(format!("Start / restart {cli_name}")).clicked()
                                {
                                    actions.push(DesktopAction::RestartCodex(id));
                                }
                                if antigravity {
                                    ui.hyperlink_to(
                                        "Antigravity installation / Google sign-in help",
                                        "https://antigravity.google/docs/install?tab=cli",
                                    );
                                }
                            });
                        });
                }
            }
        }
        for action in actions {
            if matches!(action, DesktopAction::CheckUpdates) {
                self.updates.request_check(ctx, !self.demo);
            } else if matches!(action, DesktopAction::OpenProject) {
                if let Some(project) = rfd::FileDialog::new()
                    .set_title("Open project folder")
                    .set_directory(&controller.app.project_path)
                    .pick_folder()
                    && let Err(error) = controller.switch_project(project.clone())
                {
                    controller.app.notice = error.to_string();
                }
            } else if matches!(action, DesktopAction::OpenCodex { .. })
                && !controller.app.chatgpt_ready
            {
                self.pending_cli = Some(action);
                self.pending_login = false;
            } else {
                controller.action(action);
            }
        }
        if controller.ready_to_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if controller.closing()
            || controller.app.auth_busy
            || controller.app.native_codex_busy
            || !controller.app.terminal_notices.is_empty()
        {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else {
            // Autosave drafts even when no network event or mouse movement occurs.
            ctx.request_repaint_after(Duration::from_secs(1));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.voice.shutdown();
        if let Some(controller) = self.controller.as_mut() {
            controller.request_close();
            let deadline = Instant::now() + Duration::from_secs(90);
            while !controller.ready_to_close() && Instant::now() < deadline {
                controller.poll();
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        self.runtime
            .block_on(vibe::auth::finish_pending_refreshes());
    }
}

fn pick_project() -> Option<PathBuf> {
    let mut dialog = rfd::FileDialog::new().set_title("Válassz projektmappát a CLI-hez");
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        dialog = dialog.set_directory(PathBuf::from(home));
    }
    dialog.pick_folder()
}

fn icon() -> egui::IconData {
    let size = 64usize;
    let mut rgba = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let left = (x as f32 - (12.0 + (y as f32 - 12.0) * 0.5)).abs();
            let right = (x as f32 - (52.0 - (y as f32 - 12.0) * 0.5)).abs();
            let mark = (12..=52).contains(&y) && (left < 4.0 || right < 4.0);
            rgba.extend_from_slice(if mark {
                &[126, 209, 181, 255]
            } else {
                &[13, 16, 23, 255]
            });
        }
    }
    egui::IconData {
        rgba,
        width: size as u32,
        height: size as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_project_startup_never_creates_a_controller_or_uses_the_working_directory() {
        assert!(Options::try_parse_from(["VIBE"]).unwrap().project.is_none());
        let mut app = NativeApp {
            updates: Default::default(),
            voice: Default::default(),
            controller: None,
            view: DesktopView::launcher(),
            runtime: tokio::runtime::Runtime::new().unwrap(),
            project: None,
            demo: false,
            no_save: false,
            auto_login: false,
            startup_error: String::new(),
            welcome: vibe::app::App::welcome(),
            welcome_key: None,
            pending_cli: None,
            pending_login: false,
        };
        app.initialize(&egui::Context::default());
        assert!(app.controller.is_none());
        assert!(app.project.is_none());
        assert!(app.welcome.panels.is_empty());
        assert!(app.welcome.recent_projects.is_empty());
        assert!(app.welcome.project_path.as_os_str().is_empty());
    }
}
