//! Native presentation only. Authentication, files and inference stay in the host.
use std::collections::HashMap;

use eframe::egui::{
    self, Align, Align2, Color32, FontId, Id, Layout, Pos2, Rect, RichText, Sense, Stroke, Ui,
    UiBuilder, vec2,
};

use crate::{
    app::{App, LayoutMode, ModelChoice, Panel, PanelStatus, ProviderKind, Role},
    desktop_types::DesktopAction,
};

const BG: Color32 = Color32::from_rgb(10, 13, 16);
const RAIL: Color32 = Color32::from_rgb(13, 16, 19);
const PANEL: Color32 = Color32::from_rgb(17, 21, 26);
const TERM: Color32 = Color32::from_rgb(13, 16, 19);
const INPUT: Color32 = Color32::from_rgb(27, 27, 32);
const HOVER: Color32 = Color32::from_rgb(36, 36, 42);
const LINE: Color32 = Color32::from_rgb(36, 43, 53);
const TEXT: Color32 = Color32::from_rgb(238, 239, 239);
const MUTED: Color32 = Color32::from_rgb(157, 165, 181);
const DIM: Color32 = Color32::from_rgb(112, 122, 141);
const BLUE: Color32 = Color32::from_rgb(59, 130, 246);
const ORANGE: Color32 = Color32::from_rgb(226, 150, 108);
const MINT: Color32 = Color32::from_rgb(137, 208, 180);
const GAP: f32 = 12.0;
const SHELL_INSET: i8 = 12;
const HEADER_HEIGHT: f32 = 80.0;
const PANEL_RADIUS: u8 = 12;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum RightTab {
    #[default]
    Dashboard,
    Browser,
    Changes,
}

pub struct DesktopView {
    input_scope: std::path::PathBuf,
    rules_dialog: Option<crate::rules_ui::RulesDialog>,
    file_filter: String,
    pub launcher: bool,
    right_tab: RightTab,
    dock_open: bool,
    left_ratio: f32,
    stack_ratio: f32,
    model_search: HashMap<u64, String>,
    model_drafts: HashMap<u64, String>,
    title_drafts: HashMap<u64, String>,
    group_drafts: HashMap<String, String>,
    focus_composer: Option<u64>,
    privacy_open: bool,
    openrouter_open: bool,
    openrouter_title: String,
    openrouter_model: String,
    openrouter_key: String,
    card_rects: Vec<(u64, Rect)>,
    composer_rects: HashMap<u64, Rect>,
    launcher_targets: HashMap<String, Rect>,
    terminal_regions: HashMap<u64, Rect>,
    profile_rect: Option<Rect>,
    shell_rects: Vec<Rect>,
    window_controls: Vec<Rect>,
    workspace_menu_rects: HashMap<std::path::PathBuf, Rect>,
    workspace_expanded: HashMap<std::path::PathBuf, bool>,
    workspace_panel_rects: HashMap<(std::path::PathBuf, u64), Rect>,
    provider_usage_rects: HashMap<&'static str, Rect>,
    panel_menu_rects: HashMap<u64, Rect>,
    usage_popup_rect: Option<Rect>,
    group_header_rects: HashMap<(std::path::PathBuf, Option<String>), Rect>,
    group_expanded: HashMap<(std::path::PathBuf, Option<String>), bool>,
}

impl DesktopView {
    pub fn open_rules(&mut self, ctx: &egui::Context, root: Option<std::path::PathBuf>) {
        self.rules_dialog = Some(crate::rules_ui::RulesDialog::open(ctx, root));
    }
    pub fn terminal_regions(&self) -> Vec<(u64, Rect)> {
        let mut regions: Vec<_> = self
            .terminal_regions
            .iter()
            .map(|(id, rect)| (*id, *rect))
            .collect();
        regions.sort_by_key(|(id, _)| *id);
        regions
    }

    pub fn open_files(&mut self) {
        self.right_tab = RightTab::Browser;
        self.dock_open = true;
    }
    pub fn open_changes(&mut self) {
        self.right_tab = RightTab::Changes;
        self.dock_open = true;
    }

    pub fn terminal_input_allowed(&self, app: &App) -> bool {
        !self.launcher
            && app.files.preview.is_none()
            && !self.privacy_open
            && !self.openrouter_open
            && self.rules_dialog.is_none()
            && !app.show_help
            && app.login_url.is_none()
            && app.native_codex_login_url.is_none()
    }

    pub fn launcher() -> Self {
        Self {
            launcher: true,
            ..Self::default()
        }
    }
}

impl Default for DesktopView {
    fn default() -> Self {
        Self {
            input_scope: std::path::PathBuf::new(),
            rules_dialog: None,
            file_filter: String::new(),
            launcher: false,
            right_tab: RightTab::Dashboard,
            dock_open: false,
            left_ratio: 0.5,
            stack_ratio: 0.5,
            model_search: HashMap::new(),
            model_drafts: HashMap::new(),
            title_drafts: HashMap::new(),
            group_drafts: HashMap::new(),
            focus_composer: None,
            privacy_open: false,
            openrouter_open: false,
            openrouter_title: "OpenRouter".into(),
            openrouter_model: "openrouter/auto".into(),
            openrouter_key: String::new(),
            card_rects: Vec::new(),
            composer_rects: HashMap::new(),
            launcher_targets: HashMap::new(),
            terminal_regions: HashMap::new(),
            profile_rect: None,
            shell_rects: Vec::new(),
            window_controls: Vec::new(),
            workspace_menu_rects: HashMap::new(),
            workspace_expanded: HashMap::new(),
            workspace_panel_rects: HashMap::new(),
            provider_usage_rects: HashMap::new(),
            panel_menu_rects: HashMap::new(),
            usage_popup_rect: None,
            group_header_rects: HashMap::new(),
            group_expanded: HashMap::new(),
        }
    }
}

pub fn configure(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    let mut fallback = egui::FontDefinitions::default();
    fallback.families.insert(
        egui::FontFamily::Name("semibold".into()),
        fallback.families[&egui::FontFamily::Proportional].clone(),
    );
    fallback.families.insert(
        egui::FontFamily::Name("terminal-bold".into()),
        fallback.families[&egui::FontFamily::Monospace].clone(),
    );
    ctx.set_fonts(fallback);
    #[cfg(target_os = "windows")]
    if let Some(windows) = std::env::var_os("WINDIR")
        && let Ok(bytes) =
            std::fs::read(std::path::PathBuf::from(windows).join("Fonts/segoeui.ttf"))
    {
        let mut fonts = egui::FontDefinitions::default();
        fonts
            .font_data
            .insert("ui".into(), egui::FontData::from_owned(bytes).into());
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "ui".into());
        let mut semibold = fonts.families[&egui::FontFamily::Proportional].clone();
        if let Ok(bytes) = std::fs::read(
            std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap()).join("Fonts/seguisb.ttf"),
        ) {
            fonts
                .font_data
                .insert("semibold".into(), egui::FontData::from_owned(bytes).into());
            semibold.insert(0, "semibold".into());
        }
        fonts
            .families
            .insert(egui::FontFamily::Name("semibold".into()), semibold);
        let mut terminal_bold = fonts.families[&egui::FontFamily::Monospace].clone();
        for (name, file) in [
            ("terminal-regular", "consola.ttf"),
            ("terminal-bold", "consolab.ttf"),
        ] {
            if let Ok(bytes) = std::fs::read(
                std::path::PathBuf::from(std::env::var_os("WINDIR").unwrap())
                    .join("Fonts")
                    .join(file),
            ) {
                fonts
                    .font_data
                    .insert(name.into(), egui::FontData::from_owned(bytes).into());
                if name == "terminal-regular" {
                    fonts
                        .families
                        .entry(egui::FontFamily::Monospace)
                        .or_default()
                        .insert(0, name.into());
                } else {
                    terminal_bold.insert(0, name.into());
                }
            }
        }
        fonts.families.insert(
            egui::FontFamily::Name("terminal-bold".into()),
            terminal_bold,
        );
        ctx.set_fonts(fonts);
    }
    let mut style = egui::Style {
        visuals: egui::Visuals::dark(),
        ..Default::default()
    };
    style.text_styles.insert(
        egui::TextStyle::Name("terminal-bold".into()),
        FontId::new(14.0, egui::FontFamily::Name("terminal-bold".into())),
    );
    style.animation_time = 0.16;
    style.visuals.window_shadow = egui::epaint::Shadow {
        offset: [0, 12],
        blur: 32,
        spread: 2,
        color: Color32::from_black_alpha(90),
    };
    style.visuals.override_text_color = Some(TEXT);
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = PANEL;
    style.visuals.window_stroke = Stroke::new(1.0_f32, LINE);
    style.visuals.extreme_bg_color = BG;
    style.visuals.faint_bg_color = INPUT;
    style.visuals.code_bg_color = TERM;
    style.visuals.selection.bg_fill = Color32::from_rgb(43, 64, 103);
    style.visuals.selection.stroke = Stroke::new(1.0_f32, BLUE);
    style.visuals.hyperlink_color = BLUE;
    style.visuals.widgets.noninteractive.bg_fill = PANEL;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    style.visuals.widgets.inactive.bg_fill = INPUT;
    style.visuals.widgets.inactive.weak_bg_fill = INPUT;
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(40, 47, 57));
    style.visuals.widgets.hovered.bg_fill = HOVER;
    style.visuals.widgets.hovered.weak_bg_fill = HOVER;
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, Color32::from_rgb(65, 74, 92));
    style.visuals.widgets.active.bg_fill = Color32::from_rgb(42, 55, 80);
    style.visuals.widgets.active.bg_stroke = Stroke::new(1.0_f32, BLUE);
    for visuals in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
        &mut style.visuals.widgets.noninteractive,
    ] {
        visuals.corner_radius = egui::CornerRadius::same(12);
    }
    style.spacing.item_spacing = vec2(10.0, 10.0);
    style.spacing.menu_margin = egui::Margin::same(10);
    style.spacing.button_padding = vec2(12.0, 8.0);
    style.spacing.interact_size = vec2(36.0, 36.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(16.5));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(15.5));
    style
        .text_styles
        .insert(egui::TextStyle::Small, FontId::proportional(13.5));
    style
        .text_styles
        .insert(egui::TextStyle::Heading, FontId::proportional(23.0));
    style
        .text_styles
        .insert(egui::TextStyle::Monospace, FontId::monospace(15.5));
    style.visuals.window_corner_radius = egui::CornerRadius::same(16);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(12);
    // Keep the UI at its natural scale; egui still applies the OS DPI factor.
    ctx.set_zoom_factor(1.0);
    ctx.set_style(style);
}

pub fn draw(
    ctx: &egui::Context,
    app: &mut App,
    view: &mut DesktopView,
    closing: bool,
) -> Vec<DesktopAction> {
    let mut actions = Vec::new();
    view.input_scope.clone_from(&app.project_path);
    if let Some(dialog) = &mut view.rules_dialog
        && dialog.show(ctx)
    {
        view.rules_dialog = None;
    }
    if app.panels.is_empty() {
        view.launcher = true;
        view.focus_composer = None;
    }
    view.launcher_targets.clear();
    view.terminal_regions.clear();
    view.card_rects.clear();
    view.composer_rects.clear();
    view.shell_rects.clear();
    view.window_controls.clear();
    view.provider_usage_rects.clear();
    view.panel_menu_rects.clear();
    view.usage_popup_rect = None;
    view.group_header_rects.clear();
    view.workspace_menu_rects.clear();
    view.workspace_panel_rects.clear();
    let interactive = !closing
        && app.files.preview.is_none()
        && app.login_url.is_none()
        && app.native_codex_login_url.is_none()
        && !app.show_help
        && !view.privacy_open
        && !view.openrouter_open
        && view.rules_dialog.is_none();
    let terminal_focused = !view.launcher
        && app.panels.get(app.active).is_some_and(|panel| {
            panel.provider.is_terminal()
                && !app.demo
                && ctx.memory(|memory| memory.has_focus(Id::new(("native_terminal", panel.id))))
        });
    if interactive && !egui::Popup::is_any_open(ctx) && !terminal_focused {
        shortcuts(ctx, app, view, &mut actions);
    }
    let screen_width = ctx.content_rect().width();
    egui::TopBottomPanel::top("workspace_header")
        .exact_height(HEADER_HEIGHT)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(BG)
                .inner_margin(egui::Margin::symmetric(SHELL_INSET, 12)),
        )
        .show(ctx, |ui| {
            ui.add_enabled_ui(interactive, |ui| {
                header(ui, app, view, &mut actions, screen_width)
            });
        });
    if app.sidebar_visible && screen_width >= 700.0 {
        egui::SidePanel::left("workspace_rail")
            .show_separator_line(false)
            .exact_width((screen_width * 0.145).clamp(252.0, 280.0))
            .resizable(false)
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
                left: SHELL_INSET,
                right: 0,
                top: 0,
                bottom: SHELL_INSET,
            }))
            .show(ctx, |ui| {
                ui.add_enabled_ui(interactive, |ui| {
                    projects_sidebar(ui, app, view, &mut actions)
                });
            });
    }
    if screen_width >= 1000.0 {
        egui::SidePanel::right("right_dock")
            .show_separator_line(false)
            .exact_width((screen_width * 0.22).clamp(280.0, 360.0))
            .resizable(false)
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
                left: 0,
                right: SHELL_INSET,
                top: 0,
                bottom: SHELL_INSET,
            }))
            .show(ctx, |ui| {
                ui.add_enabled_ui(interactive, |ui| {
                    framed_dock(ui, right_dock, app, view, &mut actions)
                });
            });
    }
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin {
            left: 12,
            right: 12,
            top: 0,
            bottom: SHELL_INSET,
        }))
        .show(ctx, |ui| {
            ui.add_enabled_ui(interactive, |ui| {
                if view.launcher {
                    egui::Frame::new()
                        .fill(PANEL)
                        .corner_radius(PANEL_RADIUS)
                        .inner_margin(egui::Margin::same(16))
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            launcher_center(ui, app, view, &mut actions);
                        });
                } else {
                    workspace(ui, app, view, &mut actions);
                }
            });
        });
    if app.show_help {
        help(ctx, app);
    }
    if view.privacy_open {
        privacy(ctx, view);
    }
    if view.openrouter_open {
        openrouter_settings(ctx, view, &mut actions);
    }
    if let Some(url) = app.login_url.clone() {
        login_modal(ctx, &url, closing, false, &mut actions);
    }
    if let Some(url) = app.native_codex_login_url.clone() {
        login_modal(ctx, &url, closing, true, &mut actions);
    }
    if closing {
        egui::Modal::new(Id::new("closing")).show(ctx, |ui| {
            ui.set_max_width(360.0);
            ui.horizontal(|ui| {
                ui.spinner();
                ui.heading("Closing VIBE");
            });
            ui.label("Finishing authentication and saving the session...");
        });
    }
    if !closing {
        edit_review_windows(ctx, app, &mut actions);
        crate::files_ui::preview(ctx, app, &mut actions);
    }
    actions
}

fn edit_review_windows(ctx: &egui::Context, app: &App, actions: &mut Vec<DesktopAction>) {
    for (panel_id, edit) in &app.edit_reviews {
        let mut open = true;
        let width = (ctx.content_rect().width() - 60.0).clamp(280.0, 920.0);
        egui::Window::new("Fájlmódosítás jóváhagyása")
            .id(Id::new(("agent_edit_review", panel_id)))
            .order(egui::Order::Foreground)
            .default_width(width)
            .resizable(false)
            .collapsible(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.set_width(width);
                ui.label(RichText::new(&edit.path).monospace().strong().color(BLUE));
                ui.label(
                    RichText::new(format!("Workspace: {}", app.project_name))
                        .small()
                        .color(MUTED),
                );
                ui.label("A fájl még nem változott. Csak az Alkalmazás gomb írja lemezre.");
                ui.add_space(8.0);
                egui::ScrollArea::vertical()
                    .id_salt(("edit_preview", panel_id))
                    .max_height((ctx.content_rect().height() - 280.0).clamp(100.0, 500.0))
                    .show(ui, |ui| {
                        ui.columns(2, |columns| {
                            for (column, (label, text, color)) in columns.iter_mut().zip([
                                (
                                    "Eredeti",
                                    edit.before.as_deref().unwrap_or("[Új fájl]"),
                                    MUTED,
                                ),
                                ("Javasolt", edit.after.as_str(), MINT),
                            ]) {
                                column.horizontal(|ui| {
                                    ui.label(RichText::new(label).strong());
                                    if ui.small_button("Másolás").clicked() {
                                        ctx.copy_text(text.into());
                                    }
                                });
                                let preview = crate::workspace_tools::bounded(text.into(), 24000);
                                column.add(
                                    egui::Label::new(
                                        RichText::new(preview).monospace().size(13.0).color(color),
                                    )
                                    .wrap()
                                    .selectable(true),
                                );
                            }
                        });
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Alkalmazás").clicked() {
                        actions.push(DesktopAction::ReviewEdit {
                            panel_id: *panel_id,
                            approve: true,
                        });
                    }
                    if ui.button("Elutasítás").clicked() {
                        actions.push(DesktopAction::ReviewEdit {
                            panel_id: *panel_id,
                            approve: false,
                        });
                    }
                });
            });
        if !open {
            actions.push(DesktopAction::ReviewEdit {
                panel_id: *panel_id,
                approve: false,
            });
        }
    }
}

fn shortcuts(
    ctx: &egui::Context,
    app: &mut App,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
) {
    let command = |key| egui::KeyboardShortcut::new(egui::Modifiers::CTRL, key);
    if ctx.input_mut(|input| input.consume_shortcut(&command(egui::Key::N))) {
        view.launcher = true;
    }
    if ctx.input_mut(|input| input.consume_shortcut(&command(egui::Key::O))) {
        actions.push(DesktopAction::Login);
    }
    if ctx.input_mut(|input| input.consume_shortcut(&command(egui::Key::S))) {
        actions.push(DesktopAction::Save);
    }
    let backwards = ctx.input_mut(|input| {
        input.consume_key(
            egui::Modifiers::CTRL | egui::Modifiers::SHIFT,
            egui::Key::Tab,
        )
    });
    let forwards = !backwards
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::CTRL, egui::Key::Tab));
    if (backwards || forwards) && !app.panels.is_empty() {
        app.focus_next(backwards);
        view.focus_composer = Some(app.active_panel().id);
        view.launcher = false;
    }
    if ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F1)) {
        app.show_help = true;
    }
    if let Some(panel) = app.panels.get(app.active)
        && panel.status == PanelStatus::Running
        && ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
    {
        actions.push(DesktopAction::Cancel(panel.id));
    }
}

fn header(
    ui: &mut Ui,
    app: &mut App,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
    width: f32,
) {
    let (rect, drag) =
        ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if drag.double_clicked() {
        let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
        ui.ctx()
            .send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    }
    let brand = Rect::from_min_size(rect.min, vec2(150.0, rect.height()));
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(brand)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            let home = ui.scope(logo);
            if ui
                .interact(
                    home.response.rect,
                    Id::new("workspace_home"),
                    Sense::click(),
                )
                .on_hover_text("Choose a model")
                .clicked()
            {
                view.launcher = true;
            }
            ui.label(RichText::new("VIBE").size(16.0).strong());
        },
    );
    let openai = app.chatgpt_ready;
    // Antigravity owns sign-in; its panel remains available while browser
    // authentication finishes. The popup reports the actual quota state.
    let google = app.antigravity_enabled
        || app.antigravity_usage.snapshot.is_some()
        || app
            .panels
            .iter()
            .any(|panel| panel.provider == ProviderKind::Antigravity);
    let icon_count = usize::from(openai) + usize::from(google);
    let wide_header = width >= 1600.0;
    let center_width = 254.0
        + if icon_count == 0 || wide_header {
            0.0
        } else {
            12.0 + icon_count as f32 * 84.0
        };
    let center = Rect::from_center_size(rect.center(), vec2(center_width, 44.0));
    ui.scope_builder(UiBuilder::new().max_rect(center), |ui| {
        ui.horizontal(|ui| {
            tabs(ui, true);
            if openai || google {
                ui.add_space(4.0);
            }
            if openai && !wide_header {
                provider_usage_button(ui, app, view, actions, false, false);
            }
            if google && !wide_header {
                provider_usage_button(ui, app, view, actions, true, false);
            }
        });
    });
    if wide_header {
        let usage = Rect::from_min_size(
            Pos2::new(rect.right() - 620.0, rect.top() + 5.0),
            vec2(284.0, 44.0),
        );
        ui.scope_builder(UiBuilder::new().max_rect(usage), |ui| {
            ui.horizontal(|ui| {
                if openai {
                    provider_usage_button(ui, app, view, actions, false, true);
                }
                if google {
                    provider_usage_button(ui, app, view, actions, true, true);
                }
            });
        });
    }
    let tools = Rect::from_min_max(
        Pos2::new((rect.right() - 320.0).max(rect.left() + 170.0), rect.top()),
        rect.max,
    );
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(tools)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            for (label, hint, command) in [
                ("×", "Close", egui::ViewportCommand::Close),
                (
                    "□",
                    "Maximize / restore",
                    egui::ViewportCommand::Maximized(
                        !ui.input(|input| input.viewport().maximized.unwrap_or(false)),
                    ),
                ),
                ("−", "Minimize", egui::ViewportCommand::Minimized(true)),
            ] {
                let response = ui
                    .add(
                        egui::Button::new(RichText::new(label).size(24.0).color(TEXT))
                            .frame(false)
                            .min_size(vec2(44.0, 44.0)),
                    )
                    .on_hover_text(hint);
                view.window_controls.push(response.rect);
                if response.clicked() {
                    ui.ctx().send_viewport_cmd(command);
                }
            }
            if width >= 1100.0 {
                toolbar(ui, app, view, actions);
            }
            if width < 700.0 {
                account_menu(ui, app, view, actions, true);
            }
        },
    );
}

fn usage_remaining(app: &App, google: bool) -> Option<f64> {
    let value = if google {
        app.antigravity_usage.snapshot.as_ref().and_then(|usage| {
            usage
                .models
                .iter()
                .filter_map(|model| model.remaining_percent)
                .filter(|value| value.is_finite())
                .reduce(f64::min)
        })
    } else {
        app.account_usage
            .snapshot
            .as_ref()
            .and_then(|usage| usage.primary.as_ref())
            .map(|window| window.remaining_percent)
    };
    value
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0))
}
fn usage_percent(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.0}%"))
        .unwrap_or_else(|| "—".into())
}

fn provider_usage_button(
    ui: &mut Ui,
    app: &App,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
    google: bool,
    wide: bool,
) {
    let (name, source) = if google {
        (
            "Google",
            egui::include_image!("../assets/providers/google.svg"),
        )
    } else {
        (
            "OpenAI",
            egui::include_image!("../assets/providers/openai.svg"),
        )
    };
    ui.push_id(("provider_usage", name), |ui| {
        let image = egui::Image::new(source)
            .fit_to_exact_size(vec2(22.0, 22.0))
            .alt_text(format!("{name} usage"));
        let percent = usage_percent(usage_remaining(app, google));
        let text = if wide {
            format!("{name}  {percent}")
        } else {
            percent
        };
        let response = ui
            .add(
                egui::Button::image_and_text(image, RichText::new(text).size(13.0))
                    .min_size(vec2(if wide { 132.0 } else { 80.0 }, 44.0))
                    .frame(false),
            )
            .on_hover_text(if google {
                "Google: lowest remaining model quota · last fetched data · click to refresh"
            } else {
                "OpenAI: remaining primary / 5-hour budget · last fetched data · click to refresh"
            });
        view.provider_usage_rects.insert(name, response.rect);
        let popup_id = egui::Popup::default_response_id(&response);
        // The same anchor toggles the menu closed. Closing is not a refresh.
        if response.clicked() && !egui::Popup::is_id_open(ui.ctx(), popup_id) {
            actions.push(if google {
                DesktopAction::RefreshAntigravityUsage
            } else {
                DesktopAction::RefreshUsage
            });
        }
        let popup = egui::Popup::menu(&response)
            .width(380.0)
            .gap(12.0)
            .align(egui::emath::RectAlign::BOTTOM_START)
            .frame(
                egui::Frame::popup(ui.style())
                    .fill(PANEL)
                    .corner_radius(12)
                    .inner_margin(16),
            )
            .show(|ui| {
                // Popup::width is only a default width, not a constraint. Reset
                // both bounds every frame so fill-width children cannot feed
                // their measured width back into the next popup layout.
                let body_width = (ui.ctx().content_rect().width() - 64.0).clamp(160.0, 348.0);
                ui.set_width(body_width);
                ui.set_max_width(body_width);

                ui.label(RichText::new("Account & usage").strong().size(17.0));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    let source = if google {
                        egui::include_image!("../assets/providers/google.svg")
                    } else {
                        egui::include_image!("../assets/providers/openai.svg")
                    };
                    ui.add(egui::Image::new(source).fit_to_exact_size(vec2(22.0, 22.0)));
                    ui.label(RichText::new(name).size(14.0).strong());
                });
                ui.add_space(12.0);
                egui::ScrollArea::vertical()
                    .max_height((ui.ctx().content_rect().height() - 230.0).max(100.0))
                    .show(ui, |ui| {
                        if google {
                            antigravity_usage_card(ui, app, actions);
                        } else {
                            account_usage_card(ui, app, actions);
                        }
                    });
                if google {
                    google_usage_footer(ui, app, actions);
                } else {
                    account_usage_footer(ui, app, actions);
                }
            });
        if let Some(popup) = popup {
            view.usage_popup_rect = Some(popup.response.rect);
        }
    });
}

fn logo(ui: &mut Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::hover());
    let c = rect.center();
    ui.painter().line_segment(
        [c + vec2(-7.0, -6.0), c + vec2(0.0, 7.0)],
        Stroke::new(4.0_f32, Color32::from_rgb(113, 154, 244)),
    );
    ui.painter().line_segment(
        [c + vec2(0.0, 7.0), c + vec2(7.0, -6.0)],
        Stroke::new(4.0_f32, Color32::from_rgb(77, 112, 220)),
    );
}

fn tabs(ui: &mut Ui, narrow: bool) {
    let tab_width = if narrow { 78.0 } else { 86.0 };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 2.0;
        for (label, active) in [("Agent", false), ("Code", true), ("Thread", false)] {
            let (rect, response) = ui.allocate_exact_size(vec2(tab_width, 40.0), Sense::hover());
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(14.5),
                if active { TEXT } else { DIM },
            );
            if active {
                let underline = Rect::from_min_size(
                    rect.left_bottom() + vec2(18.0, -2.0),
                    vec2(rect.width() - 36.0, 2.0),
                );
                ui.painter().rect_filled(underline, 1.0, BLUE);
            } else {
                response.on_hover_text("Coming later");
            }
        }
    });
}

fn toolbar(ui: &mut Ui, app: &mut App, view: &mut DesktopView, _actions: &mut Vec<DesktopAction>) {
    if header_icon(ui, TopIcon::Bell, "Notifications").clicked() {
        app.show_help = true;
    }
    if app.panels.len() < 8 && header_icon(ui, TopIcon::Plus, "New agent").clicked() {
        view.launcher = true;
    }
    if header_icon(ui, TopIcon::Grid, "Toggle sidebar").clicked() {
        app.sidebar_visible = !app.sidebar_visible;
    }
}

fn account_menu(
    ui: &mut Ui,
    app: &mut App,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
    compact: bool,
) {
    let signed_in = !matches!(
        app.account_label.as_str(),
        "" | "Not signed in" | "Nincs bejelentkezve"
    );
    let width = if compact {
        44.0
    } else {
        (ui.available_width() - 52.0).max(110.0)
    };
    let (rect, response) = ui.allocate_exact_size(vec2(width, 52.0), Sense::click());
    ui.painter().rect_filled(
        rect,
        10.0,
        if response.hovered() {
            HOVER
        } else {
            Color32::TRANSPARENT
        },
    );
    if compact {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "⚙",
            FontId::proportional(22.0),
            MUTED,
        );
    } else {
        view.profile_rect = Some(rect);
        let center = Pos2::new(rect.left() + 24.0, rect.center().y);
        ui.painter().circle_filled(center, 15.0, BLUE);
        let initial = if signed_in {
            app.account_label
                .chars()
                .next()
                .unwrap_or('V')
                .to_ascii_uppercase()
        } else {
            'V'
        };
        ui.painter().text(
            center,
            Align2::CENTER_CENTER,
            initial,
            FontId::proportional(15.0),
            Color32::WHITE,
        );
        let label = if signed_in {
            shorten(&app.account_label, ((width - 65.0) / 8.0).max(5.0) as usize)
        } else {
            "Sign in".into()
        };
        ui.painter().text(
            rect.min + vec2(48.0, 17.0),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(14.5),
            TEXT,
        );
        ui.painter().text(
            rect.min + vec2(48.0, 36.0),
            Align2::LEFT_CENTER,
            if app.auth_busy {
                "Connecting…"
            } else if app.chatgpt_ready {
                "ChatGPT connected"
            } else {
                "Account & settings"
            },
            FontId::proportional(11.5),
            MUTED,
        );
    }
    egui::Popup::menu(&response)
        .width(324.0)
        .align(egui::emath::RectAlign::TOP_START)
        .gap(10.0)
        .frame(
            egui::Frame::popup(ui.style())
                .fill(Color32::from_rgb(22, 22, 24))
                .stroke(Stroke::new(1.0_f32, Color32::from_rgb(54, 54, 58)))
                .corner_radius(16)
                .inner_margin(12),
        )
        .show(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            egui::ScrollArea::vertical()
                .max_height((ui.ctx().content_rect().height() - 125.0).max(180.0))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let (avatar, _) = ui.allocate_exact_size(vec2(38.0, 38.0), Sense::hover());
                        ui.painter().circle_filled(avatar.center(), 18.0, BLUE);
                        ui.painter().text(
                            avatar.center(),
                            Align2::CENTER_CENTER,
                            "V",
                            FontId::proportional(18.0),
                            Color32::WHITE,
                        );
                        ui.vertical(|ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(if signed_in {
                                        &app.account_label
                                    } else {
                                        "Welcome to VIBE"
                                    })
                                    .strong(),
                                )
                                .truncate(),
                            );
                            ui.label(
                                RichText::new(if app.chatgpt_ready {
                                    "ChatGPT subscription"
                                } else {
                                    "Account & preferences"
                                })
                                .small()
                                .color(MUTED),
                            );
                        });
                    });
                    ui.add_space(6.0);
                    ui.separator();
                    if app.native_codex_account.is_none() {
                        if app.native_codex_busy {
                            ui.horizontal(|ui| { ui.spinner(); ui.label("Connecting to Codex…"); });
                            if menu_row(ui, "Cancel Codex sign-in", false) { actions.push(DesktopAction::CancelCodexAccount); ui.close(); }
                        } else if menu_row(ui, "Continue with ChatGPT / Codex", false) {
                            actions.push(DesktopAction::ConnectCodexAccount);
                            ui.close();
                        }
                    }
                    ui.separator();
                    if menu_row(ui, "Frissítések keresése…", false) {
                        actions.push(DesktopAction::CheckUpdates); ui.close();
                    }
                    ui.label(RichText::new(format!("VIBE {}",env!("CARGO_PKG_VERSION"))).small().color(DIM));
                    if signed_in && app.native_codex_account.is_none() {
                        if menu_row(ui, "Refresh models", false) {
                            actions.push(DesktopAction::RefreshModels);
                            ui.close();
                        }
                        if menu_row(ui, "Sign in again", false) {
                            actions.push(DesktopAction::Login);
                            ui.close();
                        }
                    }
                    if app.native_codex_account.is_some() {
                        if menu_row(ui, "Reconnect Codex account", false) { actions.push(DesktopAction::ConnectCodexAccount); ui.close(); }
                        if menu_row(ui, "Disconnect Codex account", true) { actions.push(DesktopAction::DisconnectCodexAccount); ui.close(); }
                    }
                    ui.add_space(4.0);
                    section_label(ui, "GOOGLE ACCOUNT");
                    if menu_row(ui, if app.antigravity_enabled { "Open Antigravity · Google" } else { "Continue with Google · Antigravity" }, false) {
                        actions.push(DesktopAction::OpenAntigravity);
                        view.launcher = false;
                        ui.close();
                    }
                    ui.label(RichText::new("Google sign-in is saved in the system keyring and reused after restart. Antigravity installs automatically if missing.").size(11.5).color(DIM));
                    section_label(ui, "PREFERENCES");
                    if menu_row(ui, "Agent szabályok", false) {
                        view.rules_dialog = Some(crate::rules_ui::RulesDialog::open(ui.ctx(), (!app.project_path.as_os_str().is_empty()).then(|| app.project_path.clone())));
                        ui.close();
                    }
                    if menu_row(ui, "OpenRouter settings", false) {
                        view.openrouter_open = true;
                        ui.close();
                    }
                    if menu_row(ui, "Keyboard shortcuts", false) {
                        app.show_help = true;
                        ui.close();
                    }
                    if menu_row(ui, "Privacy & account", false) {
                        view.privacy_open = true;
                        ui.close();
                    }
                    if !view.launcher {
                        ui.add_space(4.0);
                        section_label(ui, "PANEL LAYOUT");
                        ui.horizontal(|ui| {
                            for mode in [LayoutMode::Focus, LayoutMode::Columns, LayoutMode::Grid] {
                                if ui
                                    .selectable_value(&mut app.layout, mode, mode.label())
                                    .clicked()
                                {
                                    app.zoomed = false;
                                }
                            }
                        });
                    }
                    ui.separator();
                    ui.add(egui::Label::new(RichText::new(&app.notice).small().color(DIM)).wrap());
                    if signed_in && app.native_codex_account.is_none() && menu_row(ui, "Sign out", true) {
                        actions.push(DesktopAction::Logout);
                        ui.close();
                    }
                });
        });
    response.on_hover_text("Account and settings");
}

fn menu_row(ui: &mut Ui, label: &str, destructive: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("menu-hover"), response.hovered(), 0.14);
    if hover > 0.0 {
        ui.painter()
            .rect_filled(rect.shrink(2.0), 9.0, HOVER.gamma_multiply(hover));
    }
    ui.painter().with_clip_rect(rect).text(
        rect.left_center() + vec2(10.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        if destructive {
            Color32::from_rgb(248, 132, 132)
        } else {
            TEXT
        },
    );
    response.clicked()
}

fn usage_meter(ui: &mut Ui, label: &str, window: Option<&crate::account_usage::UsageWindow>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(label).size(13.0).color(TEXT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let text = window
                .map(|window| {
                    format!(
                        "{:.0}% remaining",
                        window.remaining_percent.clamp(0.0, 100.0)
                    )
                })
                .unwrap_or_else(|| "Unavailable".into());
            ui.label(RichText::new(text).size(12.0).color(MUTED));
        });
    });
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
    ui.painter()
        .rect_filled(rect, 4.0, Color32::from_rgb(38, 45, 57));
    if let Some(window) = window {
        let fraction = (window.remaining_percent.clamp(0.0, 100.0) / 100.0) as f32;
        if fraction > 0.0 {
            ui.painter().rect_filled(
                Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height())),
                4.0,
                BLUE,
            );
        }
        if let Some(reset) = window.resets_at {
            ui.label(RichText::new(reset_label(reset)).size(11.5).color(DIM));
        }
    }
    ui.add_space(16.0);
}
fn account_usage_card(ui: &mut Ui, app: &App, actions: &mut Vec<DesktopAction>) {
    if let Some(account) = &app.native_codex_account
        && let Some(email) = &account.email
    {
        ui.add(egui::Label::new(RichText::new(email).size(12.0).color(DIM)).truncate());
        ui.add_space(12.0);
    }
    let usage = app.account_usage.snapshot.as_ref();
    let primary = usage.and_then(|usage| usage.primary.as_ref());
    let secondary = usage.and_then(|usage| usage.secondary.as_ref());
    usage_meter(
        ui,
        &usage_window_label(
            primary.and_then(|window| window.window_seconds),
            "5-hour limit",
        ),
        primary,
    );
    usage_meter(
        ui,
        &usage_window_label(
            secondary.and_then(|window| window.window_seconds),
            "Weekly limit",
        ),
        secondary,
    );
    if app.native_codex_account.is_none() && usage.is_none() {
        ui.label(
            RichText::new("Connect your Codex account to load its usage limits.")
                .small()
                .color(MUTED),
        );
        if app.native_codex_busy {
            ui.spinner();
            if menu_row(ui, "Cancel Codex sign-in", false) {
                actions.push(DesktopAction::CancelCodexAccount);
            }
        } else if menu_row(ui, "Connect Codex account & usage", false) {
            actions.push(DesktopAction::ConnectCodexAccount);
            ui.close();
        }
    }
    if let Some(error) = &app.account_usage.error {
        ui.label(RichText::new(error).small().color(ORANGE));
    }
}

fn account_usage_footer(ui: &mut Ui, app: &App, actions: &mut Vec<DesktopAction>) {
    let usage = app.account_usage.snapshot.as_ref();
    ui.separator();
    let text = if let Some(usage) = usage {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let minutes = now.saturating_sub(usage.fetched_at).max(0) / 60;
        if minutes == 0 {
            "Updated just now".into()
        } else {
            format!("Updated {minutes}m ago")
        }
    } else {
        "Not fetched yet".into()
    };
    let (footer, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    ui.painter().text(
        footer.left_center(),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(11.5),
        DIM,
    );
    let refresh_rect =
        Rect::from_center_size(footer.right_center() - vec2(16.0, 0.0), vec2(32.0, 32.0));
    if app.account_usage.refreshing {
        ui.put(refresh_rect, egui::Spinner::new());
    } else {
        let refresh = ui
            .interact(
                refresh_rect,
                ui.id().with("manual-usage-refresh"),
                Sense::click(),
            )
            .on_hover_text("Refresh usage");
        paint_line_icon(
            ui.painter(),
            refresh_rect.center(),
            "↻",
            if refresh.hovered() { TEXT } else { MUTED },
        );
        if refresh.clicked() {
            actions.push(DesktopAction::RefreshUsage);
        }
    }
}

fn google_usage_footer(ui: &mut Ui, app: &App, actions: &mut Vec<DesktopAction>) {
    ui.separator();
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::hover());
    let text = if let Some(usage) = &app.antigravity_usage.snapshot {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let minutes = now.saturating_sub(usage.fetched_at).max(0) / 60;
        if minutes == 0 {
            "Updated just now".into()
        } else {
            format!("Updated {minutes}m ago")
        }
    } else {
        "Not fetched yet".into()
    };
    ui.painter().text(
        rect.left_center(),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(11.5),
        DIM,
    );
    let refresh_rect =
        Rect::from_center_size(rect.right_center() - vec2(16.0, 0.0), vec2(32.0, 32.0));
    if app.antigravity_usage.refreshing {
        ui.put(refresh_rect, egui::Spinner::new());
    } else {
        let response = ui
            .interact(
                refresh_rect,
                ui.id().with("google-usage-refresh"),
                Sense::click(),
            )
            .on_hover_text("Refresh Google usage");
        paint_line_icon(ui.painter(), refresh_rect.center(), "↻", MUTED);
        if response.clicked() {
            actions.push(DesktopAction::RefreshAntigravityUsage);
        }
    }
}

fn antigravity_usage_card(ui: &mut Ui, app: &App, _actions: &mut Vec<DesktopAction>) {
    if let Some(usage) = &app.antigravity_usage.snapshot {
        if let Some(account) = &usage.account_label {
            ui.add(egui::Label::new(RichText::new(account).size(12.0).color(DIM)).truncate());
            ui.add_space(8.0);
        }
        if let Some(plan) = &usage.plan_label {
            ui.label(RichText::new(plan).size(12.0).color(DIM));
        }
        for model in &usage.models {
            ui.add_space(6.0);
            let window = model.remaining_percent.map(|remaining_percent| {
                crate::account_usage::UsageWindow {
                    remaining_percent,
                    window_seconds: None,
                    resets_at: model.reset_at,
                }
            });
            usage_meter(ui, &shorten(&model.name, 36), window.as_ref());
            if model.remaining_percent.is_none()
                && let Some(amount) = model.remaining_amount
            {
                ui.label(
                    RichText::new(format!("{amount} remaining"))
                        .small()
                        .color(MUTED),
                );
            }
            if model.reset_at.is_none()
                && let Some(reset) = &model.reset_label
            {
                ui.label(RichText::new(reset).size(11.5).color(DIM));
            }
        }
        if usage.models.is_empty() {
            ui.label(
                RichText::new("No model quotas reported.")
                    .small()
                    .color(DIM),
            );
        }
    } else {
        ui.label(
            RichText::new(if app.antigravity_usage.refreshing {
                "Loading model quotas…"
            } else {
                "Open Antigravity CLI and sign in to load model quotas."
            })
            .small()
            .color(MUTED),
        );
    }
    if let Some(error) = &app.antigravity_usage.error {
        ui.label(RichText::new(error).small().color(ORANGE));
    }
}

fn usage_window_label(seconds: Option<u64>, fallback: &str) -> String {
    match seconds {
        Some(604_800) => "Weekly limit".into(),
        Some(seconds) if seconds >= 3_600 && seconds % 3_600 == 0 => {
            format!("{}-hour limit", seconds / 3_600)
        }
        Some(seconds) if seconds >= 60 => format!("{}-minute limit", seconds / 60),
        _ => fallback.into(),
    }
}

fn reset_label(reset: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let minutes = (reset.saturating_sub(now).max(0) as u64).div_ceil(60);
    if minutes == 0 {
        "Reset due now".into()
    } else if minutes >= 1_440 {
        format!("Resets in {}d {}h", minutes / 1_440, minutes % 1_440 / 60)
    } else if minutes >= 60 {
        format!("Resets in {}h {}m", minutes / 60, minutes % 60)
    } else {
        format!("Resets in {minutes}m")
    }
}

#[derive(Clone, Copy)]
enum TopIcon {
    Grid,
    Plus,
    Bell,
}

fn header_icon(ui: &mut Ui, kind: TopIcon, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 12.0, HOVER);
    }
    let c = rect.center();
    let color = if response.hovered() { TEXT } else { MUTED };
    let stroke = Stroke::new(1.8_f32, color);
    match kind {
        TopIcon::Grid => {
            for (dx, dy) in [(-6.0, -6.0), (1.0, -6.0), (-6.0, 1.0), (1.0, 1.0)] {
                ui.painter().rect_stroke(
                    Rect::from_min_size(Pos2::new(c.x + dx, c.y + dy), vec2(6.0, 6.0)),
                    1.0,
                    stroke,
                    egui::StrokeKind::Inside,
                );
            }
        }
        TopIcon::Plus => {
            ui.painter().line_segment(
                [Pos2::new(c.x - 7.0, c.y), Pos2::new(c.x + 7.0, c.y)],
                stroke,
            );
            ui.painter().line_segment(
                [Pos2::new(c.x, c.y - 7.0), Pos2::new(c.x, c.y + 7.0)],
                stroke,
            );
        }
        TopIcon::Bell => {
            ui.painter().line_segment(
                [
                    Pos2::new(c.x - 6.0, c.y + 4.5),
                    Pos2::new(c.x + 6.0, c.y + 4.5),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    Pos2::new(c.x - 4.5, c.y + 4.5),
                    Pos2::new(c.x - 5.0, c.y - 1.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    Pos2::new(c.x + 4.5, c.y + 4.5),
                    Pos2::new(c.x + 5.0, c.y - 1.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    Pos2::new(c.x - 5.0, c.y - 1.0),
                    Pos2::new(c.x - 2.0, c.y - 6.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    Pos2::new(c.x + 5.0, c.y - 1.0),
                    Pos2::new(c.x + 2.0, c.y - 6.0),
                ],
                stroke,
            );
            ui.painter().line_segment(
                [
                    Pos2::new(c.x - 2.0, c.y - 6.0),
                    Pos2::new(c.x + 2.0, c.y - 6.0),
                ],
                stroke,
            );
        }
    }
    response.on_hover_text(hint)
}
fn projects_sidebar(
    ui: &mut Ui,
    app: &mut App,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
) {
    let frame = egui::Frame::new()
        .fill(RAIL)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(PANEL_RADIUS)
        .inner_margin(egui::Margin::symmetric(9, 11))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height((ui.available_height() - 2.0).max(120.0));
            ui.horizontal(|ui| {
                ui.label(RichText::new("Workspaces").size(14.5).strong().color(TEXT));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if icon_button(ui, "+", "Open project folder").clicked() {
                        actions.push(DesktopAction::OpenProject);
                    }
                });
            });
            ui.add_space(6.0);
            let available = (ui.available_height() - 76.0).max(80.0);
            egui::ScrollArea::vertical()
                .id_salt("project_list")
                .max_height(available)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 5.0;
                    let mut projects = app.recent_projects.clone();
                    projects.sort_by_key(|project| !project.pinned);
                    for project in projects.iter() {
                        let selected = project.path == app.project_path;
                        let expanded = *view
                            .workspace_expanded
                            .entry(project.path.clone())
                            .or_insert(true);
                        let panels = if selected {
                            app.panels
                                .iter()
                                .map(|panel| (panel.id, panel.title.clone(), panel.provider))
                                .collect::<Vec<_>>()
                        } else {
                            project.panels.clone()
                        };
                        let (rect, _) = ui
                            .allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::hover());
                        let select_rect = Rect::from_min_max(
                            rect.min + vec2(22.0, 0.0),
                            Pos2::new(rect.right() - 40.0, rect.bottom()),
                        );
                        let response = ui
                            .interact(
                                select_rect,
                                Id::new(("workspace_select", &project.path)),
                                Sense::click(),
                            )
                            .on_hover_text(project.path.display().to_string());
                        if selected {
                            ui.painter()
                                .rect_filled(rect, 10.0, Color32::from_rgb(33, 33, 40));
                        } else if response.hovered() {
                            ui.painter()
                                .rect_filled(rect, 10.0, Color32::from_rgb(25, 25, 31));
                        }
                        ui.painter().text(
                            rect.min + vec2(43.0, 21.0),
                            Align2::LEFT_CENTER,
                            shorten(
                                &project.name,
                                ((rect.width() - 94.0) / 7.4).max(4.0) as usize,
                            ),
                            FontId::proportional(14.5),
                            if selected { TEXT } else { MUTED },
                        );
                        paint_line_icon(ui.painter(), rect.min + vec2(30.0, 21.0), "folder", MUTED);
                        if project.pinned {
                            paint_line_icon(
                                ui.painter(),
                                Pos2::new(rect.right() - 52.0, rect.center().y),
                                "pin",
                                BLUE,
                            );
                        }
                        let menu_rect = Rect::from_min_size(
                            Pos2::new(rect.right() - 38.0, rect.top() + 3.0),
                            vec2(36.0, 36.0),
                        );
                        view.workspace_menu_rects
                            .insert(project.path.clone(), menu_rect);
                        let menu_response = ui.interact(
                            menu_rect,
                            Id::new(("workspace_menu", &project.path)),
                            Sense::click(),
                        );
                        if menu_response.hovered() {
                            ui.painter().rect_filled(menu_rect, 8.0, HOVER);
                        }
                        for dx in [-5.0, 0.0, 5.0] {
                            ui.painter().circle_filled(
                                menu_rect.center() + vec2(dx, 0.0),
                                1.5,
                                MUTED,
                            );
                        }
                        egui::Popup::menu(&menu_response)
                            .gap(6.0)
                            .show(|ui| workspace_menu(ui, project, view, actions));
                        response.context_menu(|ui| workspace_menu(ui, project, view, actions));
                        if response.clicked() && !selected {
                            actions.push(DesktopAction::SwitchProject(project.path.clone()));
                        }
                        if response.clicked() && selected {
                            view.launcher = false;
                        }
                        if !panels.is_empty() {
                            let toggle_rect = Rect::from_min_size(rect.min, vec2(22.0, 42.0));
                            let toggle = ui
                                .interact(
                                    toggle_rect,
                                    Id::new(("workspace_expand", &project.path)),
                                    Sense::click(),
                                )
                                .on_hover_text(if expanded {
                                    "Collapse conversations"
                                } else {
                                    "Expand conversations"
                                });
                            paint_line_icon(
                                ui.painter(),
                                toggle_rect.center(),
                                if expanded {
                                    "chevron-down"
                                } else {
                                    "chevron-right"
                                },
                                DIM,
                            );
                            if toggle.clicked() {
                                view.workspace_expanded
                                    .insert(project.path.clone(), !expanded);
                            }
                        }
                        if expanded {
                            workspace_tree(ui, app, view, actions, project, selected, &panels);
                        }
                    }
                });
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                ui.allocate_ui_with_layout(
                    vec2(ui.available_width(), 56.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        account_menu(ui, app, view, actions, false);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            account_menu(ui, app, view, actions, true);
                        });
                    },
                );
                ui.add_space(9.0);
                ui.separator();
            });
        });
    view.shell_rects.push(frame.response.rect);
}
#[allow(clippy::too_many_arguments)]
fn workspace_tree(
    ui: &mut Ui,
    app: &mut App,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
    project: &crate::app::RecentProject,
    selected: bool,
    panels: &[(u64, String, ProviderKind)],
) {
    let mut groups = vec![None];
    if selected {
        groups.extend(app.workspace_groups.iter().cloned().map(Some));
        for panel in &app.panels {
            if !groups.contains(&panel.group) {
                groups.push(panel.group.clone());
            }
        }
    }
    for group in groups {
        let key = (project.path.clone(), group.clone());
        let expanded = *view.group_expanded.entry(key.clone()).or_insert(true);
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), 36.0), Sense::click());
        let mut label_rect = rect;
        label_rect.min.x += 22.0;
        paint_line_icon(
            ui.painter(),
            label_rect.left_center() + vec2(4.0, 0.0),
            if expanded {
                "chevron-down"
            } else {
                "chevron-right"
            },
            DIM,
        );
        paint_line_icon(
            ui.painter(),
            label_rect.left_center() + vec2(24.0, 0.0),
            "folder",
            DIM,
        );
        let label = group.as_deref().unwrap_or("Ungrouped");
        ui.painter().text(
            label_rect.left_center() + vec2(40.0, 0.0),
            Align2::LEFT_CENTER,
            shorten(label, ((rect.width() - 100.0) / 7.0).max(4.0) as usize),
            FontId::proportional(13.0),
            MUTED,
        );
        let toggle = ui.interact(
            Rect::from_min_size(label_rect.min, vec2(18.0, 36.0)),
            Id::new(("group-toggle", &key)),
            Sense::click(),
        );
        view.group_header_rects.insert(key.clone(), rect);
        if response.clicked() || toggle.clicked() {
            view.group_expanded.insert(key.clone(), !expanded);
            if selected {
                app.active_group = group.clone();
                app.zoomed = false;
                view.launcher = false;
            }
        }
        if selected {
            response.context_menu(|ui| {
                if let Some(name) = &group {
                    workspace_group_menu(ui, app, view, name.clone());
                } else if ui.button("+ New group").clicked() {
                    let mut n = 1;
                    while app.workspace_groups.contains(&format!("Group {n}")) {
                        n += 1;
                    }
                    let name = format!("Group {n}");
                    app.workspace_groups.push(name.clone());
                    app.active_group = Some(name);
                    ui.close();
                }
            });
        }
        if !expanded {
            continue;
        }
        for (id, title, provider) in panels {
            if selected
                && app
                    .panels
                    .iter()
                    .find(|panel| panel.id == *id)
                    .is_some_and(|panel| panel.group != group)
            {
                continue;
            }
            let active = selected
                && app
                    .panels
                    .get(app.active)
                    .is_some_and(|panel| panel.id == *id);
            let provider_label = match provider {
                ProviderKind::Chatgpt => "OpenAI",
                ProviderKind::Antigravity => "Google",
                ProviderKind::OpenRouter => "OpenRouter",
            };
            let caption = format!("{title}  ({provider_label})");
            let (rect, child) =
                ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
            view.workspace_panel_rects
                .insert((project.path.clone(), *id), rect);
            if active || child.hovered() {
                ui.painter().rect_filled(
                    rect.shrink2(vec2(8.0, 0.0)),
                    6.0,
                    if active {
                        Color32::from_rgb(27, 33, 40)
                    } else {
                        HOVER
                    },
                );
            }
            paint_line_icon(
                ui.painter(),
                rect.left_center() + vec2(54.0, 0.0),
                "chat",
                if active { BLUE } else { DIM },
            );
            ui.painter().text(
                rect.left_center() + vec2(72.0, 0.0),
                Align2::LEFT_CENTER,
                shorten(&caption, ((rect.width() - 78.0) / 6.5).max(4.0) as usize),
                FontId::proportional(12.5),
                if active { TEXT } else { MUTED },
            );
            let child = child.on_hover_text(&caption);
            if child.clicked() {
                view.launcher = false;
                actions.push(DesktopAction::SelectProjectPanel {
                    project: project.path.clone(),
                    panel_id: *id,
                });
            }
            if selected {
                child.context_menu(|ui| {
                    ui.set_width(248.0);
                    ui.label(RichText::new("Move conversation").size(15.0).strong());
                    ui.add(egui::Label::new(RichText::new(title).size(12.0).color(DIM)).truncate());
                    ui.add_space(6.0);
                    ui.separator();
                    let mut target = app
                        .panels
                        .iter()
                        .find(|panel| panel.id == *id)
                        .and_then(|panel| panel.group.clone());
                    let before = target.clone();
                    if group_choice_row(ui, "Ungrouped", target.is_none()) {
                        target = None;
                    }
                    for name in &app.workspace_groups {
                        if group_choice_row(ui, name, target.as_ref() == Some(name)) {
                            target = Some(name.clone());
                        }
                    }
                    if target != before {
                        if let Some(panel) = app.panels.iter_mut().find(|panel| panel.id == *id) {
                            panel.group = target;
                        }
                        ui.close();
                    }
                });
            }
        }
    }
    if selected {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.add_space(40.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("+ New group").size(12.0).color(DIM))
                        .frame(false),
                )
                .clicked()
            {
                let mut n = 1;
                while app.workspace_groups.contains(&format!("Group {n}")) {
                    n += 1;
                }
                let name = format!("Group {n}");
                app.workspace_groups.push(name.clone());
                app.active_group = Some(name);
                view.launcher = false;
            }
        });
    }
}

fn workspace_menu(
    ui: &mut Ui,
    project: &crate::app::RecentProject,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
) {
    ui.set_min_width(208.0);
    ui.label(RichText::new(shorten(&project.name, 26)).strong());
    ui.separator();
    if menu_row(ui, "Workspace szabályok", false) {
        view.rules_dialog = Some(crate::rules_ui::RulesDialog::open(
            ui.ctx(),
            Some(project.path.clone()),
        ));
        ui.close();
    }
    if menu_row(
        ui,
        if project.pinned {
            "Unpin workspace"
        } else {
            "Pin workspace"
        },
        false,
    ) {
        actions.push(DesktopAction::SetProjectPinned {
            project: project.path.clone(),
            pinned: !project.pinned,
        });
        ui.close();
    }
    if menu_row(ui, "Remove from list", true) {
        actions.push(DesktopAction::RemoveProject(project.path.clone()));
        ui.close();
    }
    ui.label(
        RichText::new("Project files and saved chats are kept.")
            .size(11.5)
            .color(DIM),
    );
}

fn launcher_center(
    ui: &mut Ui,
    app: &mut App,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
) {
    let width = (ui.available_width() - 24.0).clamp(240.0, 360.0);
    let height = ui.available_height();
    egui::ScrollArea::vertical()
        .id_salt("launcher_center")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            let top = (height * 0.28).min((height - 460.0).max(24.0)).max(24.0);
            ui.add_space(top);
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - width) / 2.0).max(0.0));
                ui.allocate_ui_with_layout(
                    vec2(width, 10.0),
                    Layout::top_down(Align::Center),
                    |ui| {
                        let (rect, _) = ui.allocate_exact_size(vec2(30.0, 26.0), Sense::hover());
                        let stroke = Stroke::new(1.6_f32, MUTED);
                        ui.painter().rect_stroke(
                            Rect::from_min_size(rect.min + vec2(5.0, 2.0), vec2(20.0, 8.0)),
                            1.0,
                            stroke,
                            egui::StrokeKind::Inside,
                        );
                        for dx in [5.0, 17.0] {
                            ui.painter().rect_stroke(
                                Rect::from_min_size(rect.min + vec2(dx, 14.0), vec2(8.0, 8.0)),
                                1.0,
                                stroke,
                                egui::StrokeKind::Inside,
                            );
                        }
                        ui.add_space(10.0);
                        ui.label(
                            RichText::new(if app.project_path.as_os_str().is_empty() {
                                "Válassz CLI-t a kezdéshez".into()
                            } else {
                                format!("Start vibe coding in {}", app.project_name)
                            })
                            .size(17.0)
                            .family(egui::FontFamily::Name("semibold".into()))
                            .strong()
                            .color(TEXT),
                        );
                        ui.add_space(3.0);
                        ui.label(
                            RichText::new("Codex, Gemini / Antigravity vagy OpenRouter.")
                                .size(14.0)
                                .color(DIM),
                        );
                        ui.add_space(18.0);
                        {
                            let codex = agent_tile(ui, "Codex CLI", "", TEXT, width);
                            view.launcher_targets.insert("Codex CLI".into(), codex.rect);
                            if codex.clicked() {
                                actions.push(DesktopAction::OpenCodex {
                                    model: app
                                        .available_models
                                        .first()
                                        .map(|model| model.id.clone())
                                        .unwrap_or_else(|| "gpt-6.1-sol".into()),
                                });
                                view.launcher = false;
                            }
                        }
                        let google = agent_tile(ui, "Gemini / Antigravity CLI", "", MINT, width);
                        view.launcher_targets
                            .insert("Antigravity CLI".into(), google.rect);
                        if google.clicked() {
                            actions.push(DesktopAction::OpenAntigravity);
                            view.launcher = false;
                        }
                        ui.label(
                            RichText::new("Google sign-in opens through Antigravity CLI.")
                                .size(12.0)
                                .color(DIM),
                        );
                        let router = agent_tile(ui, "OpenRouter", "", BLUE, width);
                        view.launcher_targets
                            .insert("OpenRouter".into(), router.rect);
                        if router.clicked() {
                            view.openrouter_open = true;
                        }
                        if !app.chatgpt_ready {
                            ui.add_space(10.0);
                            ui.label(
                                RichText::new("A Codex szükség esetén bejelentkezést kér.")
                                    .size(12.5)
                                    .color(MUTED),
                            );
                        }
                        ui.add_space(16.0);
                        ui.label(
                            RichText::new(if app.project_path.as_os_str().is_empty() {
                                "A projektmappát a CLI kiválasztása után adhatod meg."
                            } else {
                                "Separate conversations. One workspace. Your choice of model."
                            })
                            .size(13.0)
                            .color(DIM),
                        );
                    },
                );
            });
            ui.add_space(24.0);
        });
}

fn agent_tile(
    ui: &mut Ui,
    name: &str,
    _model: &str,
    _color: Color32,
    width: f32,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 54.0), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("tile-hover"), response.hovered(), 0.16);
    let fill = Color32::from_rgb(
        (24.0 + 12.0 * hover) as u8,
        (24.0 + 12.0 * hover) as u8,
        (29.0 + 13.0 * hover) as u8,
    );
    ui.painter().rect_filled(rect, 12.0, fill);
    ui.painter().rect_stroke(
        rect,
        PANEL_RADIUS,
        Stroke::new(1.0_f32, if response.hovered() { DIM } else { LINE }),
        egui::StrokeKind::Inside,
    );
    let logo = if name == "Codex CLI" {
        egui::include_image!("../assets/providers/openai.svg")
    } else if name == "OpenRouter" {
        egui::include_image!("../assets/providers/openrouter.svg")
    } else {
        egui::include_image!("../assets/providers/google.svg")
    };
    egui::Image::new(logo).paint_at(
        ui,
        Rect::from_center_size(
            rect.min + vec2(24.0, 27.0),
            if name == "OpenRouter" {
                vec2(28.0, 20.5)
            } else {
                vec2(24.0, 24.0)
            },
        ),
    );
    ui.painter().text(
        rect.min + vec2(44.0, 27.0),
        Align2::LEFT_CENTER,
        name,
        FontId::new(14.5, egui::FontFamily::Name("semibold".into())),
        TEXT,
    );
    response.on_hover_text(format!("Open {}", name))
}

fn openrouter_settings(
    ctx: &egui::Context,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
) {
    let modal = egui::Modal::new(Id::new("openrouter_settings")).show(ctx, |ui| {
        ui.set_width(440.0_f32.min(ctx.content_rect().width() - 64.0));
        ui.heading("OpenRouter");
        ui.label("Kódoló ügynök a kiválasztott projekt fájljaihoz.");
        ui.label(RichText::new("Az olvasott fájlrészletek a kiválasztott OpenRouter-modellhez kerülnek. Írás csak előnézet és jóváhagyás után. Eszközhívást támogató modell szükséges.").small().color(MUTED));
        ui.add_space(12.0);
        ui.label("Panel name");
        ui.add(
            egui::TextEdit::singleline(&mut view.openrouter_title)
                .char_limit(80)
                .desired_width(f32::INFINITY),
        );
        ui.label("Model ID");
        ui.add(
            egui::TextEdit::singleline(&mut view.openrouter_model)
                .hint_text("provider/model or openrouter/auto")
                .char_limit(200)
                .desired_width(f32::INFINITY),
        );
        ui.label("API key (optional when OPENROUTER_API_KEY is set)");
        ui.add(
            egui::TextEdit::singleline(&mut view.openrouter_key)
                .password(true)
                .char_limit(8192)
                .desired_width(f32::INFINITY),
        );
        ui.label(
            RichText::new(
                "The key stays in memory for this app session. It is never saved with chats.",
            )
            .small()
            .color(MUTED),
        );
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let valid = !view.openrouter_model.trim().is_empty()
                && !view.openrouter_title.trim().is_empty();
            if ui
                .add_enabled(valid, egui::Button::new("Create panel"))
                .clicked()
            {
                if !view.openrouter_key.trim().is_empty() {
                    actions.push(DesktopAction::SetOpenRouterKey(std::mem::take(
                        &mut view.openrouter_key,
                    )));
                }
                actions.push(DesktopAction::OpenAgent {
                    title: view.openrouter_title.trim().to_owned(),
                    model: view.openrouter_model.trim().to_owned(),
                });
                view.launcher = false;
                view.openrouter_open = false;
            }
            if ui.button("Cancel").clicked() {
                view.openrouter_key.clear();
                view.openrouter_open = false;
            }
        });
    });
    if modal.should_close() {
        view.openrouter_key.clear();
        view.openrouter_open = false;
    }
}

fn framed_dock(
    ui: &mut Ui,
    add: impl FnOnce(&mut Ui, &mut App, &mut DesktopView, &mut Vec<DesktopAction>),
    app: &mut App,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
) {
    let frame = egui::Frame::new()
        .fill(RAIL)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(PANEL_RADIUS)
        .inner_margin(egui::Margin::symmetric(13, 13))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.set_height((ui.available_height() - 2.0).max(120.0));
            add(ui, app, view, actions);
        });
    view.shell_rects.push(frame.response.rect);
}

fn right_dock(
    ui: &mut Ui,
    app: &mut App,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
) {
    if !view.dock_open {
        launcher_dock(ui, app, view, actions);
        return;
    }
    ui.horizontal(|ui| {
        if ui.small_button("‹").on_hover_text("Sidebar home").clicked() {
            view.dock_open = false;
        }
        let tab_width = ((ui.available_width() - 20.0) / 3.0).clamp(44.0, 92.0);
        for (tab, label) in [
            (RightTab::Browser, "Files"),
            (RightTab::Changes, "Changes"),
            (RightTab::Dashboard, "Dashboard"),
        ] {
            let (rect, response) = ui.allocate_exact_size(vec2(tab_width, 38.0), Sense::click());
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                label,
                FontId::proportional(14.0),
                if view.right_tab == tab { TEXT } else { DIM },
            );
            if view.right_tab == tab {
                ui.painter().line_segment(
                    [rect.left_bottom(), rect.right_bottom()],
                    Stroke::new(2.0_f32, BLUE),
                );
            }
            if response.clicked() {
                view.right_tab = tab;
            }
        }
    });
    ui.add_space(10.0);
    egui::ScrollArea::vertical()
        .id_salt("right_dock")
        .auto_shrink([false, false])
        .show(ui, |ui| match view.right_tab {
            RightTab::Dashboard => dashboard(ui, app, view, actions),
            RightTab::Browser => browser(ui, app, view, actions),
            RightTab::Changes => crate::files_ui::changes(ui, app, actions),
        });
}

fn launcher_dock(
    ui: &mut Ui,
    app: &mut App,
    view: &mut DesktopView,
    _actions: &mut Vec<DesktopAction>,
) {
    let height = ui.available_height();
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if icon_button(ui, "+", "Open dashboard").clicked() {
                view.right_tab = RightTab::Dashboard;
                view.dock_open = true;
            }
        });
    });
    ui.add_space((height * 0.32).max(30.0));
    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("Open in the sidebar")
                .size(16.0)
                .family(egui::FontFamily::Name("semibold".into()))
                .strong(),
        );
        ui.add_space(4.0);
        ui.label(
            RichText::new("Keep your workspace in view while switching between chats.")
                .size(13.0)
                .color(DIM),
        );
    });
    ui.add_space(16.0);
    if dock_launch_card(ui, "Browser", "Projektfájlok és csatolmányok.", false) {
        view.right_tab = RightTab::Browser;
        view.dock_open = true;
    }
    ui.add_space(8.0);
    if dock_launch_card(
        ui,
        "Dashboard",
        "Your panels, connection and session status.",
        false,
    ) {
        view.right_tab = RightTab::Dashboard;
        view.dock_open = true;
    }
    let _ = app;
}

fn dock_launch_card(ui: &mut Ui, title: &str, description: &str, selected: bool) -> bool {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 74.0), Sense::click());
    ui.painter().rect_filled(
        rect,
        12.0,
        if response.hovered() {
            HOVER
        } else {
            Color32::from_rgb(29, 29, 29)
        },
    );
    ui.painter().rect_stroke(
        rect,
        PANEL_RADIUS,
        Stroke::new(
            1.0_f32,
            if selected {
                Color32::from_rgb(90, 92, 92)
            } else {
                LINE
            },
        ),
        egui::StrokeKind::Inside,
    );
    let icon = rect.min + vec2(25.0, 37.0);
    ui.painter().rect_filled(
        Rect::from_center_size(icon, vec2(36.0, 36.0)),
        6.0,
        Color32::from_rgb(40, 40, 40),
    );
    let stroke = Stroke::new(1.3_f32, MUTED);
    ui.painter().circle_stroke(icon, 8.0, stroke);
    if title == "Browser" {
        ui.painter()
            .circle_stroke(icon, 4.0, Stroke::new(1.0_f32, DIM));
        ui.painter().line_segment(
            [
                Pos2::new(icon.x, icon.y - 8.0),
                Pos2::new(icon.x, icon.y + 8.0),
            ],
            stroke,
        );
    } else {
        for (dx, dy) in [(-3.5, -3.5), (0.5, -3.5), (-3.5, 0.5), (0.5, 0.5)] {
            ui.painter().rect_stroke(
                Rect::from_min_size(Pos2::new(icon.x + dx, icon.y + dy), vec2(3.0, 3.0)),
                0.6,
                Stroke::new(1.0_f32, MUTED),
                egui::StrokeKind::Inside,
            );
        }
    }
    ui.painter().text(
        rect.min + vec2(52.0, 17.0),
        Align2::LEFT_TOP,
        title,
        FontId::new(15.0, egui::FontFamily::Name("semibold".into())),
        TEXT,
    );
    let text_width = (rect.width() - 58.0).max(40.0);
    let mut job = egui::text::LayoutJob::simple(
        description.to_owned(),
        FontId::proportional(12.5),
        DIM,
        text_width,
    );
    job.wrap.max_rows = 2;
    ui.painter().galley(
        rect.min + vec2(52.0, 39.0),
        ui.fonts_mut(|fonts| fonts.layout_job(job)),
        DIM,
    );
    response.on_hover_text(description).clicked()
}
fn dashboard(ui: &mut Ui, app: &mut App, view: &mut DesktopView, actions: &mut Vec<DesktopAction>) {
    ui.add_space(12.0);
    ui.label(RichText::new("Project overview").size(24.0).strong());
    ui.label(RichText::new(&app.project_name).size(14.0).color(MUTED))
        .on_hover_text(display_path(&app.project_path.to_string_lossy()));
    ui.add_space(18.0);
    let running = app
        .panels
        .iter()
        .filter(|panel| panel.status == PanelStatus::Running)
        .count();
    let saved = app
        .panels
        .iter()
        .filter(|panel| panel.terminal_saved)
        .count();
    let native = app
        .panels
        .iter()
        .filter(|panel| panel.provider.is_terminal())
        .count();
    let stats = [
        (app.panels.len(), "Conversations"),
        (running, "Working"),
        (native, "CLI panels"),
        (saved, "Saved sessions"),
    ];
    for row in stats.chunks(2) {
        ui.columns(2, |columns| {
            for (column, (value, label)) in columns.iter_mut().zip(row) {
                dock_card(column, |ui| {
                    ui.label(RichText::new(value.to_string()).size(24.0).strong());
                    ui.label(RichText::new(*label).size(12.0).color(DIM));
                });
            }
        });
        ui.add_space(8.0);
    }
    ui.add_space(20.0);
    ui.horizontal(|ui| {
        ui.label(RichText::new("Conversations").strong().size(15.0));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("+ New").clicked() {
                view.launcher = true;
            }
        });
    });
    ui.add_space(6.0);
    for panel in &app.panels {
        dock_card(ui, |ui| {
            ui.label(RichText::new(&panel.title).size(14.0).strong());
            let status = if panel.terminal_saved {
                "Saved · ready to resume"
            } else if panel.status == PanelStatus::Running {
                "Working"
            } else {
                "Ready"
            };
            ui.label(RichText::new(status).size(12.0).color(
                if panel.status == PanelStatus::Running {
                    MINT
                } else {
                    DIM
                },
            ));
        });
        ui.add_space(6.0);
    }
    ui.add_space(18.0);
    section_label(ui, "CONNECTIONS");
    ui.add_space(6.0);
    ui.label(
        RichText::new(if app.chatgpt_ready {
            "OpenAI connected"
        } else {
            "OpenAI not connected"
        })
        .small()
        .color(MUTED),
    );
    if !app.chatgpt_ready && ui.button("Continue with ChatGPT").clicked() {
        actions.push(DesktopAction::Login);
    }
    ui.label(
        RichText::new(if app.antigravity_enabled {
            "Google configured"
        } else {
            "Google not configured"
        })
        .small()
        .color(DIM),
    );
    ui.add_space(12.0);
    ui.label(
        RichText::new("OpenAI usage updates during CLI activity (up to every 30s).")
            .small()
            .color(DIM),
    );
}

fn browser(ui: &mut Ui, app: &mut App, view: &mut DesktopView, actions: &mut Vec<DesktopAction>) {
    crate::files_ui::files(ui, app, &mut view.file_filter, actions);
}

fn dock_card(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(PANEL)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(12)
        .inner_margin(egui::Margin::same(11))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

fn section_label(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .small()
            .color(DIM)
            .extra_letter_spacing(1.2),
    );
}

fn description_editor(ui: &mut Ui, panel: &mut Panel) {
    ui.add(
        egui::TextEdit::singleline(&mut panel.description)
            .id_salt(("chat_description", panel.id))
            .hint_text("Leírás hozzáadása…")
            .frame(false)
            .font(egui::TextStyle::Small)
            .char_limit(120)
            .desired_width((ui.available_width() - 125.0).clamp(55.0, 220.0)),
    )
    .on_hover_text("Chat description — saved automatically");
}

fn group_choice_row(ui: &mut Ui, label: &str, selected: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("group-choice"), response.hovered(), 0.14);
    if selected || hover > 0.0 {
        ui.painter().rect_filled(
            rect.shrink(2.0),
            8.0,
            if selected {
                Color32::from_rgb(26, 34, 46)
            } else {
                HOVER.gamma_multiply(hover)
            },
        );
    }
    let painter = ui.painter().with_clip_rect(rect);
    paint_line_icon(
        &painter,
        rect.left_center() + vec2(18.0, 0.0),
        "folder",
        if selected { BLUE } else { DIM },
    );
    painter.text(
        rect.left_center() + vec2(40.0, 0.0),
        Align2::LEFT_CENTER,
        shorten(label, 26),
        FontId::proportional(14.0),
        if selected { TEXT } else { MUTED },
    );
    if selected {
        paint_line_icon(
            &painter,
            rect.right_center() - vec2(18.0, 0.0),
            "check",
            BLUE,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn workspace_group_menu(ui: &mut Ui, app: &mut App, view: &mut DesktopView, name: String) {
    ui.set_width(252.0);
    ui.label(RichText::new("Group settings").size(15.0).strong());
    ui.add(egui::Label::new(RichText::new(&name).size(12.0).color(DIM)).truncate());
    ui.add_space(8.0);
    ui.separator();
    ui.label(RichText::new("Name").size(12.0).color(MUTED));
    let draft = view
        .group_drafts
        .entry(name.clone())
        .or_insert_with(|| name.clone());
    egui::Frame::new()
        .fill(INPUT)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(8)
        .inner_margin(8)
        .show(ui, |ui| {
            ui.add(
                egui::TextEdit::singleline(draft)
                    .frame(false)
                    .char_limit(60)
                    .desired_width(232.0),
            );
        });
    let clean: String = draft
        .chars()
        .filter(|ch| !ch.is_control())
        .collect::<String>()
        .trim()
        .to_owned();
    let valid = !clean.is_empty() && (clean == name || !app.workspace_groups.contains(&clean));
    ui.add_space(6.0);
    if ui
        .add_enabled_ui(valid, |ui| {
            action_menu_row(ui, "edit", "Rename group", false)
        })
        .inner
    {
        for group in &mut app.workspace_groups {
            if *group == name {
                *group = clean.clone();
            }
        }
        for panel in &mut app.panels {
            if panel.group.as_ref() == Some(&name) {
                panel.group = Some(clean.clone());
            }
        }
        if app.active_group.as_ref() == Some(&name) {
            app.active_group = Some(clean);
        }
        view.group_drafts.remove(&name);
        ui.close();
    }
    ui.separator();
    if action_menu_row(ui, "trash", "Remove group", true) {
        app.workspace_groups.retain(|group| group != &name);
        for panel in &mut app.panels {
            if panel.group.as_ref() == Some(&name) {
                panel.group = None;
            }
        }
        if app.active_group.as_ref() == Some(&name) {
            app.active_group = None;
        }
        view.group_drafts.remove(&name);
        ui.close();
    }
    ui.add_space(4.0);
    ui.label(
        RichText::new("Conversations are kept in Ungrouped.")
            .size(11.5)
            .color(DIM),
    );
}

fn workspace(ui: &mut Ui, app: &mut App, view: &mut DesktopView, actions: &mut Vec<DesktopAction>) {
    let visible: Vec<usize> = app
        .panels
        .iter()
        .enumerate()
        .filter(|(_, panel)| panel.group == app.active_group)
        .map(|(index, _)| index)
        .collect();
    if visible.is_empty() {
        ui.centered_and_justified(|ui| {
            if ui.button("+ New agent").clicked() {
                view.launcher = true;
            }
        });
        return;
    }
    app.active = app.active.min(app.panels.len() - 1);
    if !visible.contains(&app.active) {
        app.active = visible[0];
    }
    if !(0.2..=0.8).contains(&view.left_ratio) {
        view.left_ratio = 0.5;
    }
    if !(0.2..=0.8).contains(&view.stack_ratio) {
        view.stack_ratio = 0.5;
    }
    let mut area = ui.available_rect_before_wrap();
    let candidate = layout_rects(
        area,
        visible.len(),
        app.layout,
        view.left_ratio,
        view.stack_ratio,
    );
    let compact = candidate
        .iter()
        .any(|rect| rect.width() < 260.0 || rect.height() < 280.0);
    if compact && !app.zoomed {
        let tab_rect = Rect::from_min_size(area.min, vec2(area.width(), 36.0));
        ui.scope_builder(UiBuilder::new().max_rect(tab_rect), |ui| {
            egui::ScrollArea::horizontal()
                .id_salt("compact_tabs")
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (index, panel) in app.panels.iter().enumerate() {
                            if !visible.contains(&index) {
                                continue;
                            }
                            if ui
                                .selectable_label(index == app.active, &panel.title)
                                .clicked()
                            {
                                app.active = index;
                                view.focus_composer = Some(panel.id);
                            }
                        }
                    });
                });
        });
        area.min.y = (area.min.y + 42.0).min(area.max.y);
    }
    if !compact && !app.zoomed && app.layout == LayoutMode::Focus && visible.len() == 3 {
        splitters(ui, area, view);
    }
    let tiles: Vec<(usize, Rect)> = if app.zoomed || compact {
        vec![(app.active, area)]
    } else {
        layout_rects(
            area,
            visible.len(),
            app.layout,
            view.left_ratio,
            view.stack_ratio,
        )
        .into_iter()
        .zip(visible.iter().copied())
        .map(|(rect, index)| (index, rect))
        .collect()
    };
    let shared = Shared {
        demo: app.demo,
        models: app.available_models.clone(),
        chatgpt_ready: app.chatgpt_ready,
    };
    for (index, rect) in tiles {
        let active = index == app.active;
        let result = card(
            ui,
            rect,
            &mut app.panels[index],
            active,
            app.zoomed,
            &shared,
            view,
            actions,
        );
        if result.focus {
            app.active = index;
        }
        if result.zoom {
            app.active = index;
            app.zoomed = !app.zoomed;
        }
    }
}

fn splitters(ui: &mut Ui, area: Rect, view: &mut DesktopView) {
    let left = (area.width() - GAP) * view.left_ratio;
    let vertical = Rect::from_min_size(
        Pos2::new(area.left() + left + 2.0, area.top()),
        vec2(GAP - 4.0, area.height()),
    );
    let response = ui
        .interact(vertical, Id::new("workspace_column_split"), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        view.left_ratio =
            ((pointer.x - area.left() - GAP / 2.0) / (area.width() - GAP)).clamp(0.25, 0.75);
    }
    if response.hovered() || response.dragged() {
        ui.painter()
            .rect_filled(vertical.shrink2(vec2(2.0, 12.0)), 2.0, LINE);
    }
    let top = (area.height() - GAP) * view.stack_ratio;
    let horizontal = Rect::from_min_size(
        Pos2::new(area.left(), area.top() + top + 2.0),
        vec2(left, GAP - 4.0),
    );
    let response = ui
        .interact(horizontal, Id::new("workspace_row_split"), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical);
    if response.dragged()
        && let Some(pointer) = response.interact_pointer_pos()
    {
        view.stack_ratio =
            ((pointer.y - area.top() - GAP / 2.0) / (area.height() - GAP)).clamp(0.25, 0.75);
    }
    if response.hovered() || response.dragged() {
        ui.painter()
            .rect_filled(horizontal.shrink2(vec2(12.0, 2.0)), 2.0, LINE);
    }
}

#[derive(Default)]
struct CardResult {
    focus: bool,
    zoom: bool,
}

struct Shared {
    demo: bool,
    models: Vec<ModelChoice>,
    chatgpt_ready: bool,
}

fn terminal_panel_menu(
    ui: &mut Ui,
    panel: &mut Panel,
    shared: &Shared,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
    result: &mut CardResult,
) {
    ui.label(RichText::new(&panel.title).size(14.0).strong());
    ui.label(
        RichText::new(if panel.terminal_saved {
            "Saved session · ready to resume"
        } else {
            "Original CLI session"
        })
        .small()
        .color(DIM),
    );
    ui.add_space(8.0);
    if action_menu_row(
        ui,
        "▶",
        if panel.terminal_saved {
            "Resume session"
        } else {
            "Restart CLI"
        },
        false,
    ) {
        actions.push(DesktopAction::RestartCodex(panel.id));
        ui.close();
    }
    ui.menu_button("Rename conversation", |ui| {
        let draft = view
            .title_drafts
            .entry(panel.id)
            .or_insert_with(|| panel.title.clone());
        ui.label("Conversation name");
        ui.add(
            egui::TextEdit::singleline(draft)
                .char_limit(60)
                .desired_width(220.0),
        );
        let clean: String = draft
            .chars()
            .filter(|ch| !ch.is_control())
            .take(60)
            .collect();
        if ui
            .add_enabled(!clean.trim().is_empty(), egui::Button::new("Rename"))
            .clicked()
        {
            panel.title = clean.trim().to_owned();
            ui.close();
        }
    });
    ui.menu_button("Description", |ui| {
        ui.set_min_width(240.0);
        description_editor(ui, panel);
    });
    ui.menu_button("Model & reasoning", |ui| {
        ui.set_min_width(260.0);
        if panel.provider == ProviderKind::Antigravity {
            ui.label("Use /model inside Antigravity to change the live model.");
        } else {
            model_picker(ui, panel, shared, view, actions, result);
            ui.separator();
            for effort in crate::app::Effort::ALL {
                if ui
                    .selectable_label(panel.effort == effort, effort.label())
                    .on_hover_text(effort.description())
                    .clicked()
                {
                    actions.push(DesktopAction::SetEffort {
                        panel_id: panel.id,
                        effort,
                    });
                }
            }
            ui.label(
                RichText::new("Next launch settings. Use /model for the live session.")
                    .small()
                    .color(DIM),
            );
        }
    });
    ui.separator();
    if action_menu_row(ui, "↗", "Maximize / restore panel", false) {
        result.zoom = true;
        ui.close();
    }
    if action_menu_row(ui, "□", "Save workspace", false) {
        actions.push(DesktopAction::Save);
        ui.close();
    }
    ui.separator();
    if action_menu_row(ui, "×", "Close panel", true) {
        actions.push(DesktopAction::ClosePanel(panel.id));
        ui.close();
    }
}

fn action_menu_row(ui: &mut Ui, icon: &str, label: &str, destructive: bool) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("action-hover"), response.hovered(), 0.14);
    if hover > 0.0 {
        ui.painter()
            .rect_filled(rect.shrink(2.0), 8.0, HOVER.gamma_multiply(hover));
    }
    let color = if destructive {
        Color32::from_rgb(233, 137, 145)
    } else {
        TEXT
    };
    paint_line_icon(
        ui.painter(),
        rect.left_center() + vec2(18.0, 0.0),
        icon,
        color,
    );
    ui.painter().text(
        rect.left_center() + vec2(42.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(14.0),
        color,
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn paint_line_icon(painter: &egui::Painter, center: Pos2, icon: &str, color: Color32) {
    let stroke = Stroke::new(1.4_f32, color);
    let line = |a: (f32, f32), b: (f32, f32)| {
        painter.line_segment([center + vec2(a.0, a.1), center + vec2(b.0, b.1)], stroke);
    };
    match icon {
        "check" => {
            line((-5.0, 0.0), (-1.0, 4.0));
            line((-1.0, 4.0), (6.0, -4.0));
        }
        "edit" => {
            line((-5.0, 5.0), (5.0, -5.0));
            line((-5.0, 5.0), (-6.0, 7.0));
            line((-6.0, 7.0), (-3.0, 6.0));
            line((3.0, -6.0), (6.0, -3.0));
        }
        "trash" => {
            line((-6.0, -4.0), (6.0, -4.0));
            line((-4.0, -4.0), (-3.0, 6.0));
            line((-3.0, 6.0), (3.0, 6.0));
            line((3.0, 6.0), (4.0, -4.0));
            line((-2.0, -7.0), (2.0, -7.0));
        }

        "chevron-down" => {
            line((-3.0, -1.0), (0.0, 2.0));
            line((0.0, 2.0), (3.0, -1.0));
        }
        "chevron-right" => {
            line((-1.0, -3.0), (2.0, 0.0));
            line((2.0, 0.0), (-1.0, 3.0));
        }

        "folder" => {
            painter.add(egui::Shape::closed_line(
                vec![
                    center + vec2(-7.0, -5.0),
                    center + vec2(-1.0, -5.0),
                    center + vec2(1.0, -3.0),
                    center + vec2(7.0, -3.0),
                    center + vec2(7.0, 6.0),
                    center + vec2(-7.0, 6.0),
                ],
                stroke,
            ));
        }
        "chat" => {
            painter.rect_stroke(
                Rect::from_center_size(center, vec2(13.0, 10.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            line((-4.0, 4.0), (-6.0, 7.0));
        }
        "pin" => {
            line((-3.0, -6.0), (3.0, -6.0));
            line((-2.0, -5.0), (-2.0, 0.0));
            line((2.0, -5.0), (2.0, 0.0));
            line((-4.0, 1.0), (4.0, 1.0));
            line((0.0, 1.0), (0.0, 7.0));
        }

        "×" | "x" => {
            line((-5.0, -5.0), (5.0, 5.0));
            line((5.0, -5.0), (-5.0, 5.0));
        }
        "…" | "..." => {
            for x in [-6.0, 0.0, 6.0] {
                painter.circle_filled(center + vec2(x, 0.0), 1.3, color);
            }
        }
        "+" => {
            line((-6.0, 0.0), (6.0, 0.0));
            line((0.0, -6.0), (0.0, 6.0));
        }
        "−" => line((-6.0, 0.0), (6.0, 0.0)),
        "□" | "[ ]" => {
            painter.rect_stroke(
                Rect::from_center_size(center, vec2(12.0, 12.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        "↗" | "↙" | "<>" => {
            line((-6.0, 1.0), (-6.0, 6.0));
            line((-6.0, 6.0), (-1.0, 6.0));
            line((1.0, -6.0), (6.0, -6.0));
            line((6.0, -6.0), (6.0, -1.0));
            line((-6.0, 6.0), (-1.0, 1.0));
            line((1.0, -1.0), (6.0, -6.0));
        }
        "▶" => {
            painter.add(egui::Shape::closed_line(
                vec![
                    center + vec2(-4.0, -6.0),
                    center + vec2(6.0, 0.0),
                    center + vec2(-4.0, 6.0),
                ],
                stroke,
            ));
        }
        "↻" => {
            let points: Vec<_> = (0..24)
                .map(|n| {
                    let a = 0.2 + n as f32 / 23.0 * 5.2;
                    center + vec2(a.cos() * 6.0, a.sin() * 6.0)
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
            line((6.0, -5.0), (6.0, 1.0));
            line((6.0, 1.0), (1.0, 1.0));
        }
        _ => {
            painter.text(
                center,
                Align2::CENTER_CENTER,
                icon,
                FontId::proportional(18.0),
                color,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut Ui,
    rect: Rect,
    panel: &mut Panel,
    active: bool,
    zoomed: bool,
    shared: &Shared,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
) -> CardResult {
    let mut result = CardResult::default();
    view.card_rects.push((panel.id, rect));
    let emphasis =
        ui.ctx()
            .animate_bool_with_time(Id::new(("panel-emphasis", panel.id)), active, 0.18);
    let appearance =
        ui.ctx()
            .animate_bool_with_time(Id::new(("panel-appearance", panel.id)), true, 0.18);
    ui.painter().rect_filled(
        rect,
        PANEL_RADIUS,
        TERM.gamma_multiply(0.8 + 0.2 * appearance),
    );
    let border = Color32::from_rgb(
        (43.0 + 24.0 * emphasis) as u8,
        (43.0 + 25.0 * emphasis) as u8,
        (50.0 + 30.0 * emphasis) as u8,
    );
    ui.painter().rect_stroke(
        rect,
        PANEL_RADIUS,
        Stroke::new(1.0_f32, border),
        egui::StrokeKind::Inside,
    );
    let background = ui.interact(rect, Id::new(("card", panel.id)), Sense::click());
    result.focus |= background.clicked();
    let inner = rect.shrink2(vec2(13.0, 11.0));
    if inner.width() < 60.0 || inner.height() < 80.0 {
        return result;
    }
    if panel.provider.is_terminal() && !shared.demo {
        let panel_name = panel.title.clone();
        let cli_name = if panel.provider == ProviderKind::Antigravity {
            "Antigravity CLI"
        } else {
            "Codex CLI"
        };
        let header = Rect::from_min_size(inner.min, vec2(inner.width(), 62.0));
        let source = if panel.provider == ProviderKind::Antigravity {
            egui::include_image!("../assets/providers/google.svg")
        } else {
            egui::include_image!("../assets/providers/openai.svg")
        };
        let logo = Rect::from_min_size(header.min + vec2(5.0, 8.0), vec2(30.0, 30.0));
        egui::Image::new(source).paint_at(ui, logo);
        let title_rect = Rect::from_min_max(
            header.min + vec2(49.0, 4.0),
            Pos2::new(
                (header.right() - 124.0).max(header.left() + 80.0),
                header.top() + 28.0,
            ),
        );
        ui.scope_builder(UiBuilder::new().max_rect(title_rect), |ui| {
            ui.add(egui::Label::new(RichText::new(&panel_name).strong().size(16.0)).truncate())
                .on_hover_text(&panel_name);
        });
        let subtitle = if !panel.description.trim().is_empty() {
            panel.description.clone()
        } else {
            format!("{} · {}", cli_name, panel.model)
        };
        let detail_rect = Rect::from_min_max(
            header.min + vec2(49.0, 30.0),
            Pos2::new(
                (header.right() - 120.0).max(header.left() + 80.0),
                header.bottom() - 9.0,
            ),
        );
        ui.scope_builder(UiBuilder::new().max_rect(detail_rect), |ui| {
            ui.add(egui::Label::new(RichText::new(subtitle).size(12.0).color(DIM)).truncate());
        });
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("terminal_header", panel.id))
                .max_rect(header)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.set_clip_rect(header.intersect(ui.clip_rect()));
                if icon_button(ui, "×", &format!("Close {cli_name}")).clicked() {
                    actions.push(DesktopAction::ClosePanel(panel.id));
                }
                if icon_button(ui, if zoomed { "↙" } else { "↗" }, "Resize panel").clicked() {
                    result.zoom = true;
                }
                let response = icon_button(ui, "…", "Panel menu");
                view.panel_menu_rects.insert(panel.id, response.rect);
                egui::Popup::menu(&response)
                    .width(280.0)
                    .gap(8.0)
                    .show(|ui| {
                        terminal_panel_menu(ui, panel, shared, view, actions, &mut result);
                    });
            },
        );
        ui.painter().line_segment(
            [
                Pos2::new(rect.left() + 1.0, header.bottom()),
                Pos2::new(rect.right() - 1.0, header.bottom()),
            ],
            Stroke::new(1.0_f32, LINE),
        );
        let terminal =
            Rect::from_min_max(Pos2::new(inner.left(), header.bottom() + 1.0), inner.max);
        view.terminal_regions.insert(panel.id, terminal);
        return result;
    }
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), 62.0));
    egui::Image::new(egui::include_image!("../assets/providers/openrouter.svg")).paint_at(
        ui,
        Rect::from_center_size(header.min + vec2(19.0, 26.0), vec2(30.0, 22.0)),
    );
    let title_rect = Rect::from_min_max(
        header.min + vec2(46.0, 0.0),
        egui::pos2(
            (header.right() - 128.0).max(header.left() + 60.0),
            header.bottom(),
        ),
    );
    ui.scope_builder(UiBuilder::new().max_rect(title_rect), |ui| {
        ui.set_clip_rect(title_rect);
        ui.add(egui::Label::new(RichText::new(&panel.title).strong().size(16.0)).truncate());
        ui.add(
            egui::Label::new(
                RichText::new(format!("OpenRouter · {}", panel.model))
                    .monospace()
                    .size(12.5)
                    .color(DIM),
            )
            .truncate(),
        );
    });
    ui.scope_builder(
        UiBuilder::new()
            .max_rect(header)
            .layout(Layout::right_to_left(Align::Center)),
        |ui| {
            ui.set_clip_rect(header);
            if icon_button(ui, "×", "Panel bezárása").clicked() {
                actions.push(DesktopAction::ClosePanel(panel.id));
            }
            if icon_button(ui, if zoomed { "↙" } else { "↗" }, "Panel mérete").clicked() {
                result.zoom = true;
            }
            let response = icon_button(ui, "…", "Ügynök beállításai");
            view.panel_menu_rects.insert(panel.id, response.rect);
            egui::Popup::menu(&response)
                .width(280.0)
                .gap(8.0)
                .show(|ui| {
                    ui.label(RichText::new("VIBE Agent").strong());
                    ui.label(
                        RichText::new("OpenRouter · Projektfájlok · Jóváhagyásos írás")
                            .small()
                            .color(MUTED),
                    );
                    ui.separator();
                    ui.add_enabled_ui(panel.status != PanelStatus::Running, |ui| {
                        model_picker(ui, panel, shared, view, actions, &mut result);
                        for (provider, label) in [
                            (ProviderKind::Chatgpt, "Codex CLI"),
                            (ProviderKind::Antigravity, "Antigravity CLI"),
                        ] {
                            if (provider != ProviderKind::Chatgpt || shared.chatgpt_ready)
                                && ui.button(label).clicked()
                            {
                                actions.push(DesktopAction::SetProvider {
                                    panel_id: panel.id,
                                    provider,
                                });
                                ui.close();
                            }
                        }
                    });
                    ui.separator();
                    description_editor(ui, panel);
                    let draft = view
                        .title_drafts
                        .entry(panel.id)
                        .or_insert_with(|| panel.title.clone());
                    ui.add(
                        egui::TextEdit::singleline(draft)
                            .char_limit(60)
                            .desired_width(240.0),
                    );
                    if ui.button("Átnevezés").clicked() && !draft.trim().is_empty() {
                        panel.title = clean_draft(draft).trim().to_owned();
                        ui.close();
                    }
                    if ui.button("Mentés").clicked() {
                        actions.push(DesktopAction::Save);
                        ui.close();
                    }
                });
        },
    );
    ui.painter().line_segment(
        [
            egui::pos2(rect.left() + 1.0, header.bottom()),
            egui::pos2(rect.right() - 1.0, header.bottom()),
        ],
        Stroke::new(1.0_f32, LINE),
    );
    // Transcript, editable prompt and status share one terminal flow. A new
    // session starts near the top; longer conversations scroll as one surface.
    let history = Rect::from_min_max(
        Pos2::new(inner.left() + 14.0, header.bottom() + 20.0),
        inner.max - vec2(14.0, 12.0),
    );
    if history.height() > 0.0 {
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("history_region", panel.id))
                .max_rect(history),
            |ui| {
                ui.set_clip_rect(history.intersect(ui.clip_rect()));
                egui::ScrollArea::vertical()
                    .id_salt(("chat_history", panel.id))
                    .max_height(history.height())
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 5.0;
                        ui.spacing_mut().interact_size.y = 24.0;
                        ui.set_width(history.width() - 8.0);
                        if shared.demo {
                            ui.label(
                                RichText::new("DEMO - local transcript")
                                    .monospace()
                                    .size(12.5)
                                    .color(DIM),
                            );
                            ui.add_space(10.0);
                        }
                        if !panel
                            .messages
                            .iter()
                            .any(|message| message.role == Role::User)
                        {
                            router_terminal_welcome(ui, &view.input_scope);
                        }
                        for (index, message) in panel.messages.iter().enumerate() {
                            if message.content.trim().is_empty() {
                                continue;
                            }
                            ui.push_id(index, |ui| {
                                if message.role == Role::System {
                                    let mut lines = message.content.splitn(2, '\n');
                                    let title = lines.next().unwrap_or("");
                                    let color = if title.starts_with("OK ") {
                                        MINT
                                    } else if title.starts_with('!') {
                                        ORANGE
                                    } else {
                                        MUTED
                                    };
                                    if let Some(detail) = lines.next() {
                                        egui::CollapsingHeader::new(
                                            RichText::new(title)
                                                .monospace()
                                                .size(13.0)
                                                .color(color),
                                        )
                                        .id_salt(("tool_activity", panel.id, index))
                                        .show(ui, |ui| {
                                            message_content(ui, detail, true);
                                        });
                                    } else {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(title)
                                                    .monospace()
                                                    .size(13.0)
                                                    .color(color),
                                            )
                                            .wrap()
                                            .selectable(true),
                                        );
                                    }
                                    ui.add_space(6.0);
                                    return;
                                }
                                let (name, color) = match message.role {
                                    Role::User => ("›", BLUE),
                                    Role::Assistant => ("•", MUTED),
                                    Role::System => ("system", DIM),
                                };
                                ui.horizontal_top(|ui| {
                                    ui.spacing_mut().item_spacing.x = 10.0;
                                    ui.label(
                                        RichText::new(name)
                                            .font(router_terminal_font(ui))
                                            .color(color),
                                    );
                                    ui.vertical(|ui| {
                                        ui.set_max_width(ui.available_width());
                                        if message.role == Role::User
                                            && let Some((prompt, files)) = message
                                                .content
                                                .split_once(crate::project_files::CONTEXT_MARKER)
                                        {
                                            message_content(ui, prompt, false);
                                            egui::CollapsingHeader::new(
                                                "Elküldött fájlok (küldéskori tartalom)",
                                            )
                                            .id_salt(("sent_files", panel.id, index))
                                            .show(
                                                ui,
                                                |ui| {
                                                    message_content(ui, files, true);
                                                },
                                            );
                                        } else {
                                            message_content(ui, &message.content, false);
                                        }
                                    });
                                });
                                ui.add_space(15.0);
                            });
                        }
                        if panel.status == PanelStatus::Running {
                            ui.horizontal(|ui| {
                                ui.add(egui::Spinner::new().size(13.0).color(BLUE));
                                ui.label(
                                    RichText::new("Az ügynök dolgozik…")
                                        .monospace()
                                        .size(13.0)
                                        .color(MUTED),
                                );
                            });
                        }
                        ui.add_space(22.0);
                        composer_ui(ui, panel, view, actions, &mut result);
                        ui.add_space(12.0);
                        router_terminal_status(ui, panel, &view.input_scope);
                        ui.add_space(18.0);
                    });
            },
        );
    }
    result
}

fn model_picker(
    ui: &mut Ui,
    panel: &Panel,
    shared: &Shared,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
    result: &mut CardResult,
) {
    let selected = shared
        .models
        .iter()
        .find(|choice| choice.id == panel.model)
        .map_or(panel.model.as_str(), |choice| choice.name.as_str());
    let available = (ui.available_width() - 8.0).clamp(85.0, 310.0);
    egui::ComboBox::from_id_salt(("model_picker", panel.id))
        .width(available)
        .selected_text(
            RichText::new(shorten(selected, 40))
                .monospace()
                .size(12.5)
                .color(MUTED),
        )
        .show_ui(ui, |ui| {
            ui.set_min_width(280.0);
            ui.set_max_width(430.0);
            if panel.provider == ProviderKind::Chatgpt {
                let search = view.model_search.entry(panel.id).or_default();
                ui.add(
                    egui::TextEdit::singleline(search)
                        .hint_text("Search models...")
                        .desired_width(280.0),
                );
                if !shared.chatgpt_ready {
                    ui.label(
                        RichText::new("ChatGPT sign-in and subscription access are required.")
                            .small()
                            .color(ORANGE),
                    );
                }
                if shared.models.is_empty() {
                    ui.label("The model list has not arrived yet.");
                }
                let search = search.to_lowercase();
                egui::ScrollArea::vertical()
                    .max_height(250.0)
                    .id_salt(("catalog", panel.id))
                    .show(ui, |ui| {
                        for choice in shared.models.iter().filter(|choice| {
                            search.is_empty()
                                || choice.id.to_lowercase().contains(&search)
                                || choice.name.to_lowercase().contains(&search)
                        }) {
                            if ui
                                .selectable_label(choice.id == panel.model, &choice.name)
                                .on_hover_text(&choice.id)
                                .clicked()
                            {
                                actions.push(DesktopAction::SetModel {
                                    panel_id: panel.id,
                                    model: choice.id.clone(),
                                });
                                result.focus = true;
                                ui.close();
                            }
                        }
                    });
                ui.separator();
                if ui.button("Refresh models").clicked() {
                    actions.push(DesktopAction::RefreshModels);
                }
            } else {
                ui.label(RichText::new("OpenRouter model ID").small().color(MUTED));
                let draft = view
                    .model_drafts
                    .entry(panel.id)
                    .or_insert_with(|| panel.model.clone());
                ui.add(
                    egui::TextEdit::singleline(draft)
                        .hint_text("provider/model")
                        .desired_width(290.0)
                        .char_limit(200),
                );
                if ui
                    .add_enabled(!draft.trim().is_empty(), egui::Button::new("Apply model"))
                    .clicked()
                {
                    actions.push(DesktopAction::SetModel {
                        panel_id: panel.id,
                        model: draft.trim().to_owned(),
                    });
                    result.focus = true;
                    ui.close();
                }
                ui.separator();
                if ui
                    .selectable_label(
                        panel.model == "openrouter/auto",
                        "Automatic model selection",
                    )
                    .clicked()
                {
                    actions.push(DesktopAction::SetModel {
                        panel_id: panel.id,
                        model: "openrouter/auto".into(),
                    });
                    result.focus = true;
                    ui.close();
                }
                ui.label(
                    RichText::new("Requires an OpenRouter API key.")
                        .small()
                        .color(DIM),
                );
            }
        });
}

fn router_terminal_font(ui: &Ui) -> FontId {
    let scale = ui.ctx().pixels_per_point();
    FontId::monospace((14.0 * scale).round() / scale)
}

fn router_terminal_welcome(ui: &mut Ui, project: &std::path::Path) {
    let font = router_terminal_font(ui);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        ui.label(RichText::new(">_").font(font.clone()).color(BLUE));
        ui.label(
            RichText::new("VIBE OpenRouter")
                .font(FontId::new(
                    font.size,
                    egui::FontFamily::Name("terminal-bold".into()),
                ))
                .color(TEXT),
        );
        ui.label(
            RichText::new(format!("(v{})", env!("CARGO_PKG_VERSION")))
                .font(font.clone())
                .color(MUTED),
        );
    });
    ui.add(
        egui::Label::new(
            RichText::new(project.to_string_lossy())
                .font(font.clone())
                .color(MUTED),
        )
        .truncate(),
    );
    ui.add_space(24.0);
    ui.label(
        RichText::new("Adj feladatot, vagy használd az alábbi parancsokat:")
            .font(font.clone())
            .color(MUTED),
    );
    ui.add_space(12.0);
    for (command, description) in [
        ("/help", "parancsok és elérhető eszközök"),
        ("/model", "OpenRouter-modell kiválasztása"),
        ("/clear", "beszélgetés törlése"),
    ] {
        ui.add(
            egui::Label::new(
                RichText::new(format!("{command}  –  {description}"))
                    .font(font.clone())
                    .color(TEXT),
            )
            .wrap(),
        );
    }
    ui.add_space(16.0);
    ui.label(
        RichText::new(
            "Projektfájlok olvasása, keresése és szerkesztése.
Módosítások mentése jóváhagyás után.",
        )
        .font(font)
        .color(MUTED),
    );
}

fn router_terminal_status(ui: &mut Ui, panel: &Panel, project: &std::path::Path) {
    let font = router_terminal_font(ui);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(
            RichText::new(&panel.model)
                .font(font.clone())
                .color(Color32::from_rgb(246, 221, 166)),
        );
        ui.label(RichText::new("·").font(font.clone()).color(MUTED));
        ui.add(
            egui::Label::new(
                RichText::new(project.to_string_lossy())
                    .font(font.clone())
                    .color(MINT),
            )
            .truncate(),
        );
    });
    if panel.status != PanelStatus::Ready || panel.usage.is_some() {
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new(status_label(panel.status))
                    .font(font.clone())
                    .color(if panel.status == PanelStatus::Error {
                        ORANGE
                    } else {
                        MUTED
                    }),
            );
            if let Some(usage) = panel.usage {
                ui.label(
                    RichText::new(format!(
                        "· {} token",
                        usage.prompt_tokens.saturating_add(usage.completion_tokens)
                    ))
                    .font(font.clone())
                    .color(MUTED),
                );
            }
        });
    }
}

fn composer_ui(
    ui: &mut Ui,
    panel: &mut Panel,
    view: &mut DesktopView,
    actions: &mut Vec<DesktopAction>,
    result: &mut CardResult,
) {
    if !panel.attachments.is_empty() {
        ui.horizontal_wrapped(|ui| {
            for path in &panel.attachments {
                let name = path.rsplit('/').next().unwrap_or(path);
                if ui
                    .add_enabled(
                        panel.status != PanelStatus::Running,
                        egui::Button::new(RichText::new(format!("@{name} ×")).monospace())
                            .frame(false),
                    )
                    .on_hover_text(format!("{path}\nCsatolmány eltávolítása"))
                    .clicked()
                {
                    actions.push(DesktopAction::RemoveAttachment {
                        panel_id: panel.id,
                        path: path.clone(),
                    });
                }
            }
        });
        ui.add_space(6.0);
    }
    let id = Id::new(("composer", &view.input_scope, panel.id));
    let undo_frame = crate::input_undo::prepare(ui, id, &mut panel.input);
    let enter = ui.memory(|memory| memory.has_focus(id)) && ui.is_enabled() && ui.input_mut(|input| {
        let mut enter = false;
        input.events.retain(|event| {
            if matches!(event, egui::Event::Key { key: egui::Key::Enter, pressed: true, modifiers, .. } if *modifiers == egui::Modifiers::NONE) {
                enter = true; false
            } else { true }
        });
        enter
    });
    let font = router_terminal_font(ui);
    let prompt = ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        ui.label(RichText::new("›").font(font.clone()).color(TEXT));
        // A terminal prompt grows with the draft. No chat bubble, toolbar or
        // bottom-anchored send button; Enter sends, Shift+Enter adds a line.
        let response = ui.add(
            egui::TextEdit::multiline(&mut panel.input)
                .id(id)
                .frame(false)
                .margin(egui::Margin::ZERO)
                .hint_text("Adj feladatot az ügynöknek")
                .desired_rows(1)
                .desired_width(ui.available_width())
                .font(font)
                .text_color(Color32::from_rgb(225, 228, 235))
                .char_limit(64 * 1024),
        );
        view.composer_rects.insert(panel.id, response.rect);
        if response.clicked() || response.gained_focus() {
            result.focus = true;
        }
        if view.focus_composer == Some(panel.id) {
            response.request_focus();
            view.focus_composer = None;
        }
        if response.changed() {
            panel.input = clean_draft(&panel.input);
            panel.cursor = panel.input.len();
        }
    });
    if prompt.response.clicked() {
        result.focus = true;
    }
    if crate::input_undo::finish(ui.ctx(), id, undo_frame, &panel.input) {
        panel.cursor = panel.input.len();
    }
    let running = panel.status == PanelStatus::Running;
    if running
        && ui
            .add(
                egui::Button::new(RichText::new("■ Leállítás").monospace().color(ORANGE))
                    .frame(false),
            )
            .clicked()
    {
        actions.push(DesktopAction::Cancel(panel.id));
    }
    if enter && !running && !panel.input.trim().is_empty() {
        actions.push(DesktopAction::Send(panel.id));
        result.focus = true;
    }
}

fn message_content(ui: &mut Ui, content: &str, system: bool) {
    let mut fenced = false;
    let mut segment = String::new();
    for line in content.split('\n') {
        if line.trim_start().starts_with("```") {
            render_segment(ui, &segment, fenced, system);
            segment.clear();
            fenced = !fenced;
        } else {
            if !segment.is_empty() {
                segment.push('\n');
            }
            segment.push_str(line);
        }
    }
    render_segment(ui, &segment, fenced, system);
}

fn render_segment(ui: &mut Ui, segment: &str, code: bool, system: bool) {
    if segment.is_empty() {
        return;
    }
    let color = if system {
        MUTED
    } else if code {
        Color32::from_rgb(203, 216, 226)
    } else {
        Color32::from_rgb(225, 228, 235)
    };
    ui.add(
        egui::Label::new(
            RichText::new(segment)
                .font(router_terminal_font(ui))
                .color(color),
        )
        .selectable(true)
        .wrap(),
    );
}

fn login_modal(
    ctx: &egui::Context,
    url: &str,
    closing: bool,
    native: bool,
    actions: &mut Vec<DesktopAction>,
) {
    egui::Modal::new(Id::new(if native { "native_codex_login" } else { "chatgpt_login" })).show(ctx,|ui| {
        ui.set_width(440.0_f32.min((ctx.content_rect().width()-60.0).max(180.0)));
        ui.horizontal(|ui| {ui.spinner();ui.heading("Continue with ChatGPT");});
        ui.add_space(12.0);
        ui.label("Sign-in happens in your system browser on the OpenAI website. Choose the ChatGPT account and allow subscription access.");
        ui.add_space(10.0);
        ui.label(RichText::new("Return to this window after the browser step. VIBE never asks for or stores your ChatGPT password.").color(MUTED));
        ui.add_space(12.0);
        ui.horizontal_wrapped(|ui| {
            if ui.add_enabled(!closing,egui::Button::new("Open browser")).clicked() && safe_authorization_url(url) {
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
            if ui.button("Copy link").clicked() {ctx.copy_text(url.to_owned());}
        });
        ui.label(RichText::new("auth.openai.com - secure sign-in").small().color(DIM));
        ui.separator();
        if !closing && (ui.button("Cancel sign-in").clicked() || ctx.input_mut(|input|input.consume_key(egui::Modifiers::NONE,egui::Key::Escape))) {
            actions.push(if native { DesktopAction::CancelCodexAccount } else { DesktopAction::CancelLogin });
        }
    });
}

fn safe_authorization_url(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|url| {
        url.scheme() == "https"
            && matches!(url.host_str(), Some("auth.openai.com" | "chatgpt.com"))
            && matches!(url.path(), "/api/accounts/authorize" | "/oauth/authorize")
            && url.username().is_empty()
            && url.password().is_none()
            && url.port_or_known_default() == Some(443)
    })
}

fn privacy(ctx: &egui::Context, view: &mut DesktopView) {
    egui::Modal::new(Id::new("privacy")).show(ctx,|ui| {
        ui.set_max_width(470.0);
        ui.heading("Your workspace, separate agent threads");
        ui.add_space(10.0);
        ui.label("Each agent sends only its own messages to the selected provider. The project folder is a label and session identity; this base version does not read or modify project files.");
        ui.add_space(8.0);
        ui.label("Transcripts and drafts are saved locally. ChatGPT credentials stay in protected local storage; signing out revokes the session when possible and deletes local tokens.");
        ui.add_space(8.0);
        ui.hyperlink_to("Manage ChatGPT usage and access","https://chatgpt.com/settings/usage");
        ui.add_space(12.0);
        if ui.button("Done").clicked() || ctx.input_mut(|input|input.consume_key(egui::Modifiers::NONE,egui::Key::Escape)) {view.privacy_open=false;}
    });
}

fn help(ctx: &egui::Context, app: &mut App) {
    egui::Modal::new(Id::new("desktop_help")).show(ctx,|ui| {
        ui.set_max_width(450.0);
        ui.heading("VIBE keyboard shortcuts");
        ui.add_space(12.0);
        egui::Grid::new("desktop_shortcuts").spacing(vec2(30.0,10.0)).show(ui,|ui| {
            for (keys,description) in [("Enter","Send the prompt"),("Shift+Enter","New line in the prompt"),("Shift+drag","Select native CLI output for copying"),("Ctrl+C","Copy selected text"),("Ctrl+Tab","Next agent"),("Ctrl+Shift+Tab","Previous agent"),("Ctrl+N","New agent"),("Ctrl+O","Continue with ChatGPT"),("Ctrl+S","Save session"),("Esc","Stop the active response")] {ui.label(RichText::new(keys).monospace().color(BLUE));ui.label(description);ui.end_row();}
        });
        ui.add_space(14.0);
        ui.label(RichText::new("Agent model and provider are editable in each agent header. Drag the splitters to resize panes.").color(MUTED));
        if ui.button("Back to workspace").clicked() || ctx.input_mut(|input|input.consume_key(egui::Modifiers::NONE,egui::Key::Escape)||input.consume_key(egui::Modifiers::NONE,egui::Key::F1)) {app.show_help=false;}
    });
}

fn icon_button(ui: &mut Ui, text: &str, hint: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
    let hover =
        ui.ctx()
            .animate_bool_with_time(response.id.with("icon-hover"), response.hovered(), 0.12);
    if hover > 0.0 {
        ui.painter()
            .rect_filled(rect.shrink(3.0), 10.0, HOVER.gamma_multiply(hover));
    }
    paint_line_icon(
        ui.painter(),
        rect.center(),
        text,
        if response.hovered() { TEXT } else { MUTED },
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint)
}

fn status_label(status: PanelStatus) -> &'static str {
    match status {
        PanelStatus::Ready => "ready",
        PanelStatus::Running => "running...",
        PanelStatus::Error => "request failed",
        PanelStatus::Cancelled => "stopped",
    }
}

fn display_path(path: &str) -> String {
    if let Some(network) = path.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{network}")
    } else {
        path.strip_prefix("\\\\?\\").unwrap_or(path).to_owned()
    }
}

fn shorten(text: &str, length: usize) -> String {
    if text.chars().count() <= length {
        text.to_owned()
    } else {
        format!(
            "{}...",
            text.chars()
                .take(length.saturating_sub(3))
                .collect::<String>()
        )
    }
}

fn clean_draft(text: &str) -> String {
    let normalized = text.replace("\r\n", "\n");
    let mut clean: String = normalized
        .chars()
        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t')
        .collect();
    if clean.len() > 64 * 1024 {
        let mut end = 64 * 1024;
        while !clean.is_char_boundary(end) {
            end -= 1;
        }
        clean.truncate(end);
    }
    clean
}

fn layout_rects(
    area: Rect,
    count: usize,
    mode: LayoutMode,
    left_ratio: f32,
    stack_ratio: f32,
) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![area];
    }
    if mode == LayoutMode::Focus && count == 3 {
        let left_width = ((area.width() - GAP).max(0.0) * left_ratio).max(0.0);
        let top_height = ((area.height() - GAP).max(0.0) * stack_ratio).max(0.0);
        let left = Rect::from_min_size(area.min, vec2(left_width, area.height()));
        return vec![
            Rect::from_min_size(left.min, vec2(left.width(), top_height)),
            Rect::from_min_max(
                Pos2::new(
                    left.left(),
                    (left.top() + top_height + GAP).min(left.bottom()),
                ),
                left.max,
            ),
            Rect::from_min_max(
                Pos2::new(
                    (area.left() + left_width + GAP).min(area.right()),
                    area.top(),
                ),
                area.max,
            ),
        ];
    }
    if mode == LayoutMode::Columns || count == 2 {
        return partitions(area, count, true);
    }
    let columns = (count as f32).sqrt().ceil() as usize;
    let rows = count.div_ceil(columns);
    let mut result = Vec::with_capacity(count);
    for row in partitions(area, rows, false) {
        result.extend(partitions(row, columns.min(count - result.len()), true));
    }
    result
}

fn partitions(area: Rect, count: usize, horizontal: bool) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let extent = if horizontal {
        area.width()
    } else {
        area.height()
    };
    let gap = GAP.min(extent / (count as f32));
    let size = ((extent - gap * (count - 1) as f32) / count as f32).max(0.0);
    (0..count)
        .map(|index| {
            let offset = index as f32 * (size + gap);
            if horizontal {
                Rect::from_min_size(area.min + vec2(offset, 0.0), vec2(size, area.height()))
            } else {
                Rect::from_min_size(area.min + vec2(0.0, offset), vec2(area.width(), size))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Vec2;
    use std::path::PathBuf;

    #[test]
    fn three_cards_have_independent_regions_and_right_card_is_tall() {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(1200.0, 860.0));
        let rects = layout_rects(area, 3, LayoutMode::Focus, 0.5, 0.5);
        assert_eq!(rects.len(), 3);
        assert!(rects[0].bottom() < rects[1].top());
        assert!(rects[2].height() > rects[0].height() * 2.0);
        for count in 1..=8 {
            for mode in [LayoutMode::Focus, LayoutMode::Columns, LayoutMode::Grid] {
                let rects = layout_rects(area, count, mode, 0.5, 0.5);
                assert_eq!(rects.len(), count);
                for (index, rect) in rects.iter().enumerate() {
                    assert!(area.contains_rect(*rect));
                    for other in rects.iter().skip(index + 1) {
                        assert!(!rect.intersects(*other));
                    }
                }
            }
        }
    }

    fn frame(
        ctx: &egui::Context,
        app: &mut App,
        view: &mut DesktopView,
        size: Vec2,
        events: Vec<egui::Event>,
    ) -> Vec<DesktopAction> {
        let mut actions = Vec::new();
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ctx| {
                actions = draw(ctx, app, view, false);
            },
        );
        actions
    }

    #[test]
    fn shell_panels_have_room_below_header_and_on_both_edges() {
        for size in [
            vec2(1280.0, 800.0),
            vec2(1500.0, 960.0),
            vec2(2560.0, 1440.0),
        ] {
            let ctx = egui::Context::default();
            configure(&ctx);
            let mut app = App::new(PathBuf::from("spacing-test"), true);
            let mut view = DesktopView::default();
            for _ in 0..2 {
                frame(&ctx, &mut app, &mut view, size, vec![]);
            }
            assert_eq!(view.shell_rects.len(), 2);
            let rail_right = (size.x * 0.20).clamp(240.0, 340.0);
            assert!(
                view.shell_rects[0].right() <= rail_right + 0.5,
                "sidebar content expands past its clipping edge: {:?}, {rail_right}",
                view.shell_rects[0]
            );
            for rect in view
                .shell_rects
                .iter()
                .chain(view.card_rects.iter().map(|(_, rect)| rect))
            {
                assert!(
                    rect.top() >= HEADER_HEIGHT - 1.0,
                    "panel touches titlebar: {rect:?}"
                );
                assert!(
                    rect.left() >= 11.0 && rect.right() <= size.x - 11.0,
                    "panel outside horizontal inset: {rect:?}, {size:?}"
                );
                assert!(
                    rect.bottom() <= size.y - 10.0,
                    "panel below bottom inset: {rect:?}, {size:?}"
                );
            }
            assert_eq!(view.window_controls.len(), 3);
            for rect in &view.window_controls {
                assert!(rect.width() >= 44.0 && rect.height() >= 44.0);
            }
            let profile = view.profile_rect.unwrap();
            assert!(profile.bottom() < size.y - 12.0);
            assert!(profile.height() >= 52.0);
        }
    }

    #[test]
    fn workspace_groups_filter_cards_and_new_chats_inherit_the_group() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("groups-test"), true);
        app.workspace_groups = vec!["Frontend".into(), "Backend".into()];
        app.panels[0].group = Some("Frontend".into());
        app.panels[1].group = Some("Backend".into());
        app.active_group = Some("Backend".into());
        let expected = app.panels[1].id;
        let mut view = DesktopView::default();
        frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
        assert_eq!(view.card_rects.len(), 1);
        assert_eq!(view.card_rects[0].0, expected);
        assert_eq!(app.active_panel().id, expected);
        assert!(app.add_panel("API".into(), "model".into()));
        assert_eq!(app.active_panel().group.as_deref(), Some("Backend"));
        app.active_group = Some("Frontend".into());
        frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
        assert_eq!(view.card_rects.len(), 1);
        assert_eq!(view.card_rects[0].0, app.panels[0].id);
        assert!(app.project_path.ends_with("groups-test"));
    }

    #[test]
    fn workspace_children_select_saved_and_live_panels_and_collapse() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("workspace-tree"), true);
        app.panels[0].provider = ProviderKind::Antigravity;
        let current = app.project_path.clone();
        let live_id = app.panels[0].id;
        let other = PathBuf::from("saved-workspace");
        app.recent_projects.push(crate::app::RecentProject {
            name: "Saved workspace".into(),
            path: other.clone(),
            agents: 1,
            pinned: false,
            panels: vec![(42, "Codex CLI".into(), ProviderKind::Chatgpt)],
        });
        let mut view = DesktopView::default();
        for _ in 0..2 {
            frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
        }
        let target = view.workspace_panel_rects[&(current.clone(), live_id)].center();
        let actions = click(&ctx, &mut app, &mut view, target);
        assert!(actions.iter().any(|action| matches!(action,
            DesktopAction::SelectProjectPanel { project, panel_id } if project == &current && *panel_id == live_id)));
        let target = view.workspace_panel_rects[&(other.clone(), 42)].center();
        let actions = click(&ctx, &mut app, &mut view, target);
        assert!(actions.iter().any(|action| matches!(action,
            DesktopAction::SelectProjectPanel { project, panel_id: 42 } if project == &other)));
        let menu = view.workspace_menu_rects[&current];
        let target = Pos2::new(
            view.workspace_panel_rects[&(current.clone(), live_id)].left() + 11.0,
            menu.center().y,
        );
        let actions = click(&ctx, &mut app, &mut view, target);
        assert!(actions.is_empty());
        frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
        assert!(!view.workspace_panel_rects.contains_key(&(current, live_id)));
        assert!(view.workspace_panel_rects.contains_key(&(other, 42)));
    }

    #[test]
    fn profile_does_not_query_usage_and_workspace_actions_are_separate_from_selection() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("menu-test"), true);
        app.account_label = "test@example.invalid".into();
        app.chatgpt_ready = true;
        let path = app.project_path.clone();
        app.recent_projects.clear();
        app.recent_projects.push(crate::app::RecentProject {
            name: "Example workspace".into(),
            path: path.clone(),
            agents: 1,
            pinned: true,
            panels: Vec::new(),
        });
        let mut view = DesktopView::default();
        let size = vec2(1500.0, 950.0);
        for _ in 0..2 {
            frame(&ctx, &mut app, &mut view, size, vec![]);
        }
        let position = view.profile_rect.unwrap().center();
        let actions = click(&ctx, &mut app, &mut view, position);
        assert!(actions.iter().all(|action| !matches!(
            action,
            DesktopAction::RefreshUsage | DesktopAction::RefreshAntigravityUsage
        )));
        assert!(egui::Popup::is_any_open(&ctx));
        // Close the profile without selecting a menu action.
        click(&ctx, &mut app, &mut view, Pos2::new(700.0, 100.0));
        let position = view.workspace_menu_rects[&path].center();
        let actions = click(&ctx, &mut app, &mut view, position);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, DesktopAction::SwitchProject(_)))
        );
        assert!(egui::Popup::is_any_open(&ctx));
        assert_eq!(usage_window_label(Some(18_000), "Limit"), "5-hour limit");
        assert_eq!(usage_window_label(Some(604_800), "Limit"), "Weekly limit");
    }

    #[test]
    fn remembered_google_setup_shows_usage_without_an_open_terminal() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("native-test"), true);
        app.antigravity_enabled = true;
        app.panels
            .retain(|panel| panel.provider != ProviderKind::Antigravity);
        let mut view = DesktopView::default();
        frame(&ctx, &mut app, &mut view, vec2(1360.0, 880.0), vec![]);
        assert!(view.provider_usage_rects.contains_key("Google"));
        assert!(app.antigravity_usage.snapshot.is_none());
    }

    #[test]
    fn empty_workspace_returns_to_launcher_without_creating_a_panel() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("empty-workspace"), true);
        app.panels.clear();
        app.active = 0;
        let mut view = DesktopView {
            launcher: false,
            ..DesktopView::default()
        };
        for _ in 0..3 {
            frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
            assert!(view.launcher);
            assert!(app.panels.is_empty());
            assert!(view.card_rects.is_empty());
            assert!(view.terminal_regions.is_empty());
        }
    }

    #[test]
    fn native_ui_handles_small_and_large_windows_without_panic() {
        for size in [
            vec2(320.0, 350.0),
            vec2(720.0, 540.0),
            vec2(1360.0, 880.0),
            vec2(1600.0, 1000.0),
        ] {
            let ctx = egui::Context::default();
            configure(&ctx);
            let mut app = App::new(PathBuf::from("native-test"), true);
            let mut view = DesktopView::default();
            assert!(frame(&ctx, &mut app, &mut view, size, vec![]).is_empty());
            assert!(!view.card_rects.is_empty());
            if size.x >= 1360.0 {
                assert_eq!(view.card_rects.len(), 3);
                assert_eq!(view.composer_rects.len(), 3);
            } else {
                assert!(matches!(view.card_rects.len(), 1 | 3));
            }
            app.login_url =
                Some("https://auth.openai.com/api/accounts/authorize?state=synthetic".into());
            frame(&ctx, &mut app, &mut view, size, vec![]);
            app.login_url = None;
            app.show_help = true;
            frame(&ctx, &mut app, &mut view, size, vec![]);
            app.show_help = false;
            view.launcher = true;
            frame(&ctx, &mut app, &mut view, size, vec![]);
            assert!(view.card_rects.is_empty());
        }
    }

    #[test]
    fn provider_logos_open_only_their_own_usage_without_overlapping_window_buttons() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("provider-header"), true);
        let mut view = DesktopView::default();
        let size = vec2(1500.0, 950.0);
        frame(&ctx, &mut app, &mut view, size, vec![]);
        assert!(view.provider_usage_rects.is_empty());
        app.chatgpt_ready = true;
        app.panels[0].provider = ProviderKind::Antigravity;
        for _ in 0..2 {
            frame(&ctx, &mut app, &mut view, size, vec![]);
        }
        let openai = view.provider_usage_rects["OpenAI"];
        let google = view.provider_usage_rects["Google"];
        assert!(openai.right() < google.left());
        assert!(openai.width() >= 44.0 && google.width() >= 44.0);
        let actions = click(&ctx, &mut app, &mut view, openai.center());
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, DesktopAction::RefreshUsage))
        );
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, DesktopAction::RefreshAntigravityUsage))
        );
        assert!(egui::Popup::is_any_open(&ctx));
        click(&ctx, &mut app, &mut view, Pos2::new(700.0, 100.0));
        let google = view.provider_usage_rects["Google"];
        let actions = click(&ctx, &mut app, &mut view, google.center());
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, DesktopAction::RefreshAntigravityUsage))
        );
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, DesktopAction::RefreshUsage))
        );
        for size in [
            vec2(1000.0, 700.0),
            vec2(1280.0, 800.0),
            vec2(2560.0, 1440.0),
        ] {
            frame(&ctx, &mut app, &mut view, size, vec![]);
            for logo in view.provider_usage_rects.values() {
                assert!(logo.top() >= 12.0 && logo.bottom() < HEADER_HEIGHT);
                for control in &view.window_controls {
                    assert!(!logo.intersects(*control));
                }
            }
        }
    }

    #[test]
    fn usage_refreshes_only_when_opened_not_when_closed_or_left_open() {
        for provider in ["OpenAI", "Google"] {
            let ctx = egui::Context::default();
            configure(&ctx);
            let mut app = App::new(PathBuf::from("usage-open-only"), true);
            app.chatgpt_ready = true;
            app.antigravity_enabled = true;
            let mut view = DesktopView::default();
            let size = vec2(1500.0, 950.0);
            let is_refresh = |action: &DesktopAction| {
                matches!(
                    action,
                    DesktopAction::RefreshUsage | DesktopAction::RefreshAntigravityUsage
                )
            };
            for _ in 0..3 {
                assert!(
                    !frame(&ctx, &mut app, &mut view, size, vec![])
                        .iter()
                        .any(is_refresh)
                );
            }
            for _ in 0..2 {
                let position = view.provider_usage_rects[provider].center();
                let actions = click(&ctx, &mut app, &mut view, position);
                assert_eq!(
                    actions.iter().filter(|action| is_refresh(action)).count(),
                    1
                );
                assert!(actions.iter().any(|action| match provider {
                    "OpenAI" => matches!(action, DesktopAction::RefreshUsage),
                    _ => matches!(action, DesktopAction::RefreshAntigravityUsage),
                }));
                assert!(egui::Popup::is_any_open(&ctx));
                for _ in 0..5 {
                    assert!(
                        !frame(&ctx, &mut app, &mut view, size, vec![])
                            .iter()
                            .any(is_refresh)
                    );
                }
                let position = view.provider_usage_rects[provider].center();
                let actions = click(&ctx, &mut app, &mut view, position);
                assert!(
                    !actions.iter().any(is_refresh),
                    "Closing must not request usage"
                );
                assert!(!egui::Popup::is_any_open(&ctx));
            }
        }
    }

    #[test]
    fn google_usage_card_renders_missing_and_zero_quotas_without_network() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("google-usage-ui"), true);
        app.antigravity_usage.snapshot = Some(crate::antigravity_usage::AntigravityUsage {
            models: vec![
                crate::antigravity_usage::ModelQuota {
                    name: "Gemini test model with a long display name and quota window".into(),
                    remaining_percent: Some(0.0),
                    remaining_amount: None,
                    reset_at: Some(2_000_000_000),
                    reset_label: None,
                },
                crate::antigravity_usage::ModelQuota {
                    name: "Unknown quota".into(),
                    remaining_percent: None,
                    remaining_amount: Some(42),
                    reset_at: None,
                    reset_label: None,
                },
            ],
            fetched_at: 1000,
            account_label: None,
            plan_label: None,
        });
        let mut actions = Vec::new();
        let mut bounds = Rect::NOTHING;
        for _ in 0..2 {
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(350.0, 700.0))),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.set_max_width(324.0);
                        bounds = ui
                            .scope(|ui| antigravity_usage_card(ui, &app, &mut actions))
                            .response
                            .rect;
                    });
                },
            );
        }
        assert!(bounds.right() <= 342.0, "usage card overflows: {bounds:?}");
        assert!(actions.is_empty());
        assert_eq!(
            app.antigravity_usage.snapshot.as_ref().unwrap().models[0].remaining_percent,
            Some(0.0)
        );
        assert!(
            app.antigravity_usage.snapshot.as_ref().unwrap().models[1]
                .remaining_percent
                .is_none()
        );
    }

    #[test]
    fn antigravity_launcher_is_independent_of_chatgpt_login() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("google-test"), true);
        app.chatgpt_ready = false;
        let mut view = DesktopView::launcher();
        let size = vec2(1500.0, 950.0);
        for _ in 0..2 {
            frame(&ctx, &mut app, &mut view, size, vec![]);
        }
        assert!(view.launcher_targets.contains_key("Codex CLI"));
        let target = view.launcher_targets["Antigravity CLI"].center();
        let actions = click(&ctx, &mut app, &mut view, target);
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, DesktopAction::OpenAntigravity))
        );
        assert!(!view.launcher);
        app.demo = false;
        app.panels[0].provider = ProviderKind::Antigravity;
        let id = app.panels[0].id;
        frame(&ctx, &mut app, &mut view, size, vec![]);
        assert!(view.terminal_regions.contains_key(&id));
        assert!(!view.composer_rects.contains_key(&id));
    }

    #[test]
    fn usage_popup_remains_bounded_after_hundreds_of_open_frames() {
        use crate::account_usage::{AccountUsage, UsageWindow};
        for size in [vec2(1500.0, 950.0), vec2(2560.0, 1440.0)] {
            let ctx = egui::Context::default();
            configure(&ctx);
            let mut app = App::new(PathBuf::from("usage-bounds"), true);
            app.chatgpt_ready = true;
            app.native_codex_account = Some(crate::codex_account::AccountInfo {
                email: Some("a-very-long-account-address@example.invalid".into()),
                plan_type: None,
            });
            app.account_usage.snapshot = Some(AccountUsage {
                primary: Some(UsageWindow {
                    remaining_percent: 70.0,
                    window_seconds: Some(18000),
                    resets_at: Some(2_000_000_000),
                }),
                secondary: Some(UsageWindow {
                    remaining_percent: 95.0,
                    window_seconds: Some(604800),
                    resets_at: Some(2_000_000_000),
                }),
                fetched_at: 0,
                plan_type: None,
            });
            let mut view = DesktopView::default();
            for _ in 0..3 {
                frame(&ctx, &mut app, &mut view, size, vec![]);
            }
            let pos = view.provider_usage_rects["OpenAI"].center();
            for pressed in [true, false] {
                frame(
                    &ctx,
                    &mut app,
                    &mut view,
                    size,
                    vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                );
            }
            let mut initial = None;
            for n in 0..300 {
                assert!(
                    frame(&ctx, &mut app, &mut view, size, vec![]).is_empty(),
                    "no automatic refresh while open"
                );
                let rect = view.usage_popup_rect.expect("usage popup stays open");
                assert!(rect.width() <= 400.0, "popup grew at frame {n}: {rect:?}");
                assert!(
                    rect.height() < size.y - 40.0,
                    "popup grew taller than viewport"
                );
                if n == 20 {
                    initial = Some(rect.size());
                }
                if n > 20 {
                    assert!(
                        (rect.size() - initial.unwrap()).length() < 1.0,
                        "popup size drifted after layout settled"
                    );
                }
            }
        }
    }

    #[test]
    fn whole_group_row_selects_and_toggles_the_group() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("group-row-click"), true);
        app.workspace_groups.push("Frontend".into());
        app.panels[1].group = Some("Frontend".into());
        let path = app.project_path.clone();
        let id = app.panels[1].id;
        let key = (path.clone(), Some("Frontend".into()));
        let mut view = DesktopView::default();
        for _ in 0..3 {
            frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
        }
        app.zoomed = true;
        let pos = view.group_header_rects[&key].center();
        assert!(click(&ctx, &mut app, &mut view, pos).is_empty());
        frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
        assert_eq!(app.active_group.as_deref(), Some("Frontend"));
        assert!(!app.zoomed, "group selection restores the full group view");
        assert!(!view.group_expanded[&key]);
        assert!(!view.workspace_panel_rects.contains_key(&(path.clone(), id)));
        let pos = view.group_header_rects[&key].center();
        click(&ctx, &mut app, &mut view, pos);
        frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
        assert!(view.group_expanded[&key]);
        assert!(view.workspace_panel_rects.contains_key(&(path, id)));
    }

    #[test]
    fn header_budget_uses_primary_remaining_and_preserves_zero_and_unknown() {
        use crate::account_usage::{AccountUsage, UsageWindow};
        let mut app = App::new(PathBuf::from("usage-fixture"), true);
        assert_eq!(usage_percent(usage_remaining(&app, false)), "—");
        app.account_usage.snapshot = Some(AccountUsage {
            primary: Some(UsageWindow {
                remaining_percent: 72.0,
                window_seconds: Some(18000),
                resets_at: None,
            }),
            secondary: Some(UsageWindow {
                remaining_percent: 86.0,
                window_seconds: Some(604800),
                resets_at: None,
            }),
            fetched_at: 0,
            plan_type: None,
        });
        assert_eq!(usage_percent(usage_remaining(&app, false)), "72%");
        app.account_usage
            .snapshot
            .as_mut()
            .unwrap()
            .primary
            .as_mut()
            .unwrap()
            .remaining_percent = 0.0;
        assert_eq!(usage_percent(usage_remaining(&app, false)), "0%");
        app.account_usage.snapshot.as_mut().unwrap().primary = None;
        assert_eq!(
            usage_remaining(&app, false),
            None,
            "weekly quota must not impersonate five-hour budget"
        );
    }

    #[test]
    fn workspace_tree_shows_all_groups_but_group_toggle_hides_only_its_children() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("group-tree-fixture"), true);
        app.workspace_groups.push("Frontend".into());
        app.panels[1].group = Some("Frontend".into());
        let root = app.project_path.clone();
        let first = app.panels[0].id;
        let second = app.panels[1].id;
        let mut view = DesktopView::default();
        for _ in 0..2 {
            assert!(frame(&ctx, &mut app, &mut view, vec2(1600.0, 1000.0), vec![]).is_empty());
        }
        assert!(
            view.workspace_panel_rects
                .contains_key(&(root.clone(), first))
        );
        assert!(
            view.workspace_panel_rects
                .contains_key(&(root.clone(), second)),
            "inactive groups must keep their chats in the sidebar"
        );
        view.group_expanded
            .insert((root.clone(), Some("Frontend".into())), false);
        frame(&ctx, &mut app, &mut view, vec2(1600.0, 1000.0), vec![]);
        assert!(
            view.workspace_panel_rects
                .contains_key(&(root.clone(), first))
        );
        assert!(!view.workspace_panel_rects.contains_key(&(root, second)));
    }

    #[test]
    fn redesigned_native_menu_opens_without_launching_or_refreshing_any_provider() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("native-menu-test"), true);
        app.demo = false;
        app.panels.truncate(2);
        app.panels[0].provider = ProviderKind::Chatgpt;
        app.panels[1].provider = ProviderKind::Antigravity;
        app.layout = LayoutMode::Columns;
        let first = app.panels[0].id;
        let mut view = DesktopView::default();
        let size = vec2(1600.0, 1000.0);
        for _ in 0..3 {
            assert!(frame(&ctx, &mut app, &mut view, size, vec![]).is_empty());
        }
        let anchor = view.panel_menu_rects[&first];
        let body = view.terminal_regions[&first];
        assert!(
            anchor.bottom() < body.top(),
            "menu must stay above terminal input"
        );
        assert!(anchor.width() >= 36.0, "comfortable click target");
        let actions = click(&ctx, &mut app, &mut view, anchor.center());
        assert!(
            actions.is_empty(),
            "opening menu must not launch CLI or refresh usage"
        );
        assert!(egui::Popup::is_any_open(&ctx));
        assert!(!view.composer_rects.contains_key(&first));
    }

    #[test]
    fn codex_panels_expose_a_terminal_region_without_a_chat_composer() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("terminal-test"), true);
        app.demo = false;
        app.panels[1].provider = ProviderKind::Chatgpt;
        let codex_id = app.panels[1].id;
        let router_id = app.panels[0].id;
        let mut view = DesktopView::default();
        frame(&ctx, &mut app, &mut view, vec2(1600.0, 1000.0), vec![]);
        assert!(view.terminal_regions.contains_key(&codex_id));
        assert!(!view.composer_rects.contains_key(&codex_id));
        assert!(view.composer_rects.contains_key(&router_id));
        assert!(view.terminal_regions[&codex_id].height() > 100.0);
        view.launcher = true;
        frame(&ctx, &mut app, &mut view, vec2(1600.0, 1000.0), vec![]);
        assert!(view.terminal_regions.is_empty());
    }

    #[test]
    fn native_composer_typing_and_send_stay_in_the_selected_panel() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("native-test"), true);
        let mut view = DesktopView {
            focus_composer: Some(app.panels[1].id),
            ..Default::default()
        };
        let size = vec2(1600.0, 1000.0);
        frame(&ctx, &mut app, &mut view, size, vec![]);
        frame(
            &ctx,
            &mut app,
            &mut view,
            size,
            vec![egui::Event::Text("independent second message".into())],
        );
        assert_eq!(app.panels[1].input, "independent second message");
        assert!(app.panels[0].input.is_empty());
        assert!(app.panels[2].input.is_empty());
        let actions = frame(
            &ctx,
            &mut app,
            &mut view,
            size,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(
            actions
                .iter()
                .any(|action| matches!(action,DesktopAction::Send(id)if *id==app.panels[1].id))
        );
        assert_eq!(
            actions
                .iter()
                .filter(|action| matches!(action, DesktopAction::Send(_)))
                .count(),
            1
        );
    }

    #[test]
    fn router_inline_prompt_grows_for_shift_enter_and_enter_still_sends() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("inline-router-test"), true);
        app.demo = false;
        app.panels.truncate(1);
        app.panels[0].provider = ProviderKind::OpenRouter;
        app.panels[0].messages.clear();
        let id = app.panels[0].id;
        let mut view = DesktopView {
            focus_composer: Some(id),
            ..Default::default()
        };
        let size = vec2(1600.0, 1000.0);
        for _ in 0..3 {
            frame(&ctx, &mut app, &mut view, size, vec![]);
        }
        let initial = view.composer_rects[&id];
        let card = view
            .card_rects
            .iter()
            .find(|(panel, _)| *panel == id)
            .unwrap()
            .1;
        assert!(
            initial.bottom() < card.center().y + 80.0,
            "New-session input must follow the welcome text, not the panel bottom"
        );
        frame(
            &ctx,
            &mut app,
            &mut view,
            size,
            vec![egui::Event::Text("Első sor".into())],
        );
        let actions = frame(
            &ctx,
            &mut app,
            &mut view,
            size,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::SHIFT,
            }],
        );
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, DesktopAction::Send(_)))
        );
        frame(
            &ctx,
            &mut app,
            &mut view,
            size,
            vec![egui::Event::Text("Második sor".into())],
        );
        assert_eq!(app.panels[0].input, "Első sor\nMásodik sor");
        assert!(view.composer_rects[&id].height() > initial.height());
        let actions = frame(
            &ctx,
            &mut app,
            &mut view,
            size,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, DesktopAction::Send(panel) if *panel == id))
        );
    }

    #[test]
    fn launcher_offers_all_clis_before_authentication_and_opens_router_settings() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(PathBuf::from("native-test"), true);
        let mut view = DesktopView {
            launcher: true,
            ..Default::default()
        };
        frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
        assert!(view.card_rects.is_empty());
        assert!(!app.chatgpt_ready);
        assert!(!view.openrouter_open);
        assert_eq!(view.launcher_targets.len(), 3);
        let target = view.launcher_targets["OpenRouter"];
        assert!(target.height() >= 44.0);
        assert!(view.launcher_targets.contains_key("Codex CLI"));
        click(&ctx, &mut app, &mut view, target.center());
        assert!(view.openrouter_open);
        view.openrouter_key = "synthetic-test-secret".into();
        frame(
            &ctx,
            &mut app,
            &mut view,
            vec2(1500.0, 950.0),
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(!view.openrouter_open);
        assert!(view.openrouter_key.is_empty());
        app.chatgpt_ready = true;
        frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
        assert_eq!(view.launcher_targets.len(), 3);
        let target = view.launcher_targets["Codex CLI"];
        assert!(target.height() >= 44.0);
        let actions = click(&ctx, &mut app, &mut view, target.center());
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, DesktopAction::OpenCodex { .. }))
        );
        assert!(!view.launcher);
    }

    #[test]
    fn fresh_install_welcome_is_empty_and_opens_only_the_cli_chooser() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::welcome();
        let mut view = DesktopView::launcher();
        for _ in 0..12 {
            let actions = frame(&ctx, &mut app, &mut view, vec2(1500.0, 950.0), vec![]);
            assert!(actions.is_empty());
            assert!(app.project_path.as_os_str().is_empty());
            assert!(app.recent_projects.is_empty());
            assert!(app.panels.is_empty());
            assert!(view.launcher);
            assert!(!view.dock_open);
            assert_eq!(view.launcher_targets.len(), 3);
            assert!(view.card_rects.is_empty());
        }
    }

    fn click(
        ctx: &egui::Context,
        app: &mut App,
        view: &mut DesktopView,
        position: Pos2,
    ) -> Vec<DesktopAction> {
        let size = vec2(1500.0, 950.0);
        frame(
            ctx,
            app,
            view,
            size,
            vec![
                egui::Event::PointerMoved(position),
                egui::Event::PointerButton {
                    pos: position,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        frame(
            ctx,
            app,
            view,
            size,
            vec![egui::Event::PointerButton {
                pos: position,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
    }

    #[test]
    fn login_opening_accepts_only_official_authorization_endpoint() {
        assert!(safe_authorization_url(
            "https://auth.openai.com/api/accounts/authorize?state=test"
        ));
        for url in [
            "http://auth.openai.com/api/accounts/authorize",
            "https://auth.openai.com.attacker.invalid/api/accounts/authorize",
            "https://auth.openai.com@attacker.invalid/api/accounts/authorize",
            "https://auth.openai.com:443/api/accounts/oauth/token",
        ] {
            assert!(!safe_authorization_url(url));
        }
    }

    #[test]
    fn file_browser_loads_only_when_opened_and_does_not_repeat_pending_requests() {
        use crate::project_files::{Load, Request};
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut app = App::new(std::path::PathBuf::from("file-browser-ui-test"), true);
        let mut view = DesktopView::default();
        let size = vec2(1500.0, 950.0);
        let actions = frame(&ctx, &mut app, &mut view, size, vec![]);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, DesktopAction::Browse(_)))
        );
        view.open_files();
        let actions = frame(&ctx, &mut app, &mut view, size, vec![]);
        assert!(actions.iter().any(
            |action| matches!(action,DesktopAction::Browse(Request::Directory(path)) if path==".")
        ));
        app.files.directories.insert(".".into(), Load::Loading);
        for _ in 0..10 {
            let actions = frame(&ctx, &mut app, &mut view, size, vec![]);
            assert!(
                !actions
                    .iter()
                    .any(|action| matches!(action, DesktopAction::Browse(_)))
            );
        }
        view.open_changes();
        let actions = frame(&ctx, &mut app, &mut view, size, vec![]);
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, DesktopAction::Browse(Request::Changes)))
        );
        app.files.changes = Some(Load::Loading);
        let actions = frame(&ctx, &mut app, &mut view, size, vec![]);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, DesktopAction::Browse(_)))
        );
        assert!(view.terminal_input_allowed(&app));
        app.files.preview = Some((
            Request::File("main.rs".into()),
            Load::Ready("preview".into()),
        ));
        assert!(!view.terminal_input_allowed(&app));
    }
}
