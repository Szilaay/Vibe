//! On-demand file tree, Git changes and read-only previews.
use crate::{
    app::{App, PanelStatus, ProviderKind},
    desktop_types::DesktopAction,
    project_files::{Load, Request},
};
use eframe::egui::{self, Color32, RichText, Ui};
const MUTED: Color32 = Color32::from_rgb(157, 165, 181);
const BLUE: Color32 = Color32::from_rgb(59, 130, 246);
const GREEN: Color32 = Color32::from_rgb(137, 208, 180);
const ORANGE: Color32 = Color32::from_rgb(226, 150, 108);

fn file_row(ui: &mut Ui, label: &str, color: Color32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 32.0), egui::Sense::click());
    if response.hovered() || response.has_focus() {
        ui.painter()
            .rect_filled(rect, 6.0, ui.visuals().widgets.hovered.bg_fill);
    }
    ui.painter().with_clip_rect(rect).text(
        rect.left_center() + egui::vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::monospace(13.5),
        color,
    );
    response
}

pub fn files(ui: &mut Ui, app: &App, filter: &mut String, actions: &mut Vec<DesktopAction>) {
    if app.project_path.as_os_str().is_empty() {
        ui.label("A fájlböngészőhöz előbb válassz CLI-t és projektmappát.");
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Projektfájlok").size(18.0).strong());
        if ui.small_button("Frissítés").clicked() {
            actions.push(DesktopAction::RefreshFiles);
        }
    });
    ui.add(egui::Label::new(RichText::new(&app.project_name).small().color(MUTED)).truncate());
    ui.add(
        egui::TextEdit::singleline(filter)
            .hint_text("Szűrés a betöltött nevekre…")
            .desired_width(ui.available_width())
            .char_limit(120),
    );
    ui.add_space(8.0);
    let mut budget = 700;
    directory(
        ui,
        app,
        ".",
        &filter.to_lowercase(),
        0,
        &mut budget,
        actions,
    );
    if budget == 0 {
        ui.label(
            RichText::new("700 sor a korlát. Csukj össze néhány mappát.")
                .small()
                .color(MUTED),
        );
    }
}

fn directory(
    ui: &mut Ui,
    app: &App,
    path: &str,
    filter: &str,
    depth: usize,
    budget: &mut usize,
    actions: &mut Vec<DesktopAction>,
) {
    match app.files.directories.get(path) {
        None => {
            actions.push(DesktopAction::Browse(Request::Directory(path.into())));
            ui.spinner();
        }
        Some(Load::Loading) => {
            ui.spinner();
        }
        Some(Load::Failed(error)) => {
            ui.label(RichText::new(error).small().color(ORANGE));
            if ui.small_button("Újrapróbálás").clicked() {
                actions.push(DesktopAction::Browse(Request::Directory(path.into())));
            }
        }
        Some(Load::Ready(data)) => {
            if data.entries.is_empty() {
                ui.label(
                    RichText::new("Üres vagy kizárt mappa.")
                        .small()
                        .color(MUTED),
                );
            }
            for entry in &data.entries {
                if *budget == 0 {
                    break;
                }
                *budget -= 1;
                if entry.directory {
                    egui::CollapsingHeader::new(RichText::new(&entry.name).size(14.0))
                        .id_salt(("project_files", &app.project_path, &entry.path))
                        .show(ui, |ui| {
                            if depth < 12 {
                                directory(ui, app, &entry.path, filter, depth + 1, budget, actions);
                            } else {
                                ui.label("A fa mélységi korlátja: 12 mappa.");
                            }
                        });
                } else if (filter.is_empty() || entry.name.to_lowercase().contains(filter))
                    && file_row(ui, &entry.name, ui.visuals().text_color())
                        .on_hover_text(&entry.path)
                        .clicked()
                {
                    actions.push(DesktopAction::Browse(Request::File(entry.path.clone())));
                }
            }
            if data.truncated {
                ui.label(
                    RichText::new("Legfeljebb 500 bejegyzés mappánként; a lista csonkolt.")
                        .small()
                        .color(MUTED),
                );
            }
        }
    }
}

pub fn changes(ui: &mut Ui, app: &App, actions: &mut Vec<DesktopAction>) {
    if app.project_path.as_os_str().is_empty() {
        ui.label("A Git-nézethez előbb válassz CLI-t és projektmappát.");
        return;
    }
    ui.horizontal(|ui| {
        ui.label(RichText::new("Git-változások").size(18.0).strong());
        if ui.small_button("Frissítés").clicked() {
            actions.push(DesktopAction::Browse(Request::Changes));
        }
    });
    ui.label(
        RichText::new("Helyi állapot · kézi frissítés · csak olvasható")
            .small()
            .color(MUTED),
    );
    ui.add_space(10.0);
    match &app.files.changes {
        None => {
            actions.push(DesktopAction::Browse(Request::Changes));
            ui.spinner();
        }
        Some(Load::Loading) => {
            ui.spinner();
        }
        Some(Load::Failed(error)) => {
            ui.label(RichText::new(error).color(ORANGE));
        }
        Some(Load::Ready(data)) => {
            if data.entries.is_empty() {
                ui.label("Nincs megjeleníthető változás a workspace-ben.");
            }
            for entry in &data.entries {
                let color = if entry.status.contains('D') {
                    ORANGE
                } else if entry.status == "??" || entry.status.contains('A') {
                    GREEN
                } else {
                    BLUE
                };
                if file_row(ui, &format!("{}  {}", entry.status, entry.path), color)
                    .on_hover_text(format!(
                        "{}\nBal jel: index; jobb jel: munkafájl. ?? = új fájl. Kattintás: diff.",
                        entry.path
                    ))
                    .clicked()
                {
                    actions.push(DesktopAction::Browse(Request::Diff(entry.path.clone())));
                }
            }
            if data.truncated {
                ui.label("A lista 500 bejegyzésnél csonkolt.");
            }
        }
    }
}

pub fn preview(ctx: &egui::Context, app: &App, actions: &mut Vec<DesktopAction>) {
    let Some((request, data)) = &app.files.preview else {
        return;
    };
    let (path, diff) = match request {
        Request::File(path) => (path, false),
        Request::Diff(path) => (path, true),
        _ => return,
    };
    let modal=egui::Modal::new(egui::Id::new(("project_file_preview",&app.project_path,request))).show(ctx,|ui| {
        ui.set_width((ctx.content_rect().width()-60.0).clamp(280.0,1080.0));
        ui.horizontal(|ui| {ui.heading(if diff{"Változások előnézete"}else{"Fájl előnézete"});
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center),|ui|{if ui.button("Bezárás").clicked(){actions.push(DesktopAction::CloseFilePreview);}});
        });
        ui.label(RichText::new(path).monospace().color(BLUE));
        match data {
            Load::Loading=>{ui.spinner();}
            Load::Failed(error)=>{ui.label(RichText::new(error).color(ORANGE));if ui.button("Újrapróbálás").clicked(){actions.push(DesktopAction::Browse(request.clone()));}}
            Load::Ready(text)=>{
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Tartalom másolása").clicked(){ctx.copy_text(text.clone());}
                    if !diff {
                        let target=app.panels.get(app.active).filter(|panel|panel.provider==ProviderKind::OpenRouter && panel.status!=PanelStatus::Running);
                        let enabled=target.is_some_and(|panel|panel.attachments.len()<8 && !panel.attachments.contains(path)) && text.len()<=64*1024;
                        if ui.add_enabled(enabled,egui::Button::new("Csatolás az aktív OpenRouter-prompt mellé")).clicked() && let Some(panel)=target{actions.push(DesktopAction::AttachFile{panel_id:panel.id,path:path.clone()});}
                    }
                });
                if !diff{ui.label(RichText::new("A fájl tartalmát küldéskor olvassa be az app. Összesen 8 fájl / 64 KiB; küldéskor a választott modellszolgáltatóhoz kerül.").small().color(MUTED));}
                ui.separator();
                let lines:Vec<_>=text.lines().collect();
                egui::ScrollArea::both().id_salt(("project_file_text",request)).max_height((ctx.content_rect().height()-280.0).clamp(100.0,650.0))
                    .auto_shrink([false,false]).show_rows(ui,18.0,lines.len(),|ui,range| {
                        ui.spacing_mut().item_spacing.y=0.0;
                        for index in range {
                            let line=lines[index];
                            let color=if diff && line.starts_with('+'){GREEN}else if diff && line.starts_with('-'){ORANGE}else if diff && (line.starts_with('@') || line.starts_with('#')){BLUE}else{ui.visuals().text_color()};
                            let content=if diff{line.into()}else{format!("{:>5}  {line}",index+1)};
                            ui.add(egui::Label::new(RichText::new(content).monospace().size(13.0).color(color)).wrap_mode(egui::TextWrapMode::Extend).selectable(true));
                        }
                    });
            }
        }
    });
    if modal.should_close() {
        actions.push(DesktopAction::CloseFilePreview);
    }
}
