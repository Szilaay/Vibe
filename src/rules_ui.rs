//! Explicitly opened settings; bounded disk work never runs on the UI thread.
use crate::scoped_rules::{self, Rules};
use eframe::egui::{self, Id, RichText, vec2};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver},
};

struct Document {
    rules: Rules,
    loaded: bool,
    pending: Option<Receiver<Result<Rules, String>>>,
    error: Option<String>,
    saved: bool,
}
impl Document {
    fn load(
        ctx: &egui::Context,
        path: impl FnOnce() -> anyhow::Result<PathBuf> + Send + 'static,
    ) -> Self {
        let mut doc = Self {
            rules: Rules::default(),
            loaded: false,
            pending: None,
            error: None,
            saved: false,
        };
        doc.job(ctx, move || scoped_rules::load(&path()?));
        doc
    }
    fn job(
        &mut self,
        ctx: &egui::Context,
        work: impl FnOnce() -> anyhow::Result<Rules> + Send + 'static,
    ) {
        let (send, receive) = mpsc::channel();
        let repaint = ctx.clone();
        self.pending = Some(receive);
        self.error = None;
        std::thread::spawn(move || {
            let _ = send.send(work().map_err(|error| format!("{error:#}")));
            repaint.request_repaint();
        });
    }
    fn poll(&mut self) {
        if let Some(receive) = &self.pending {
            match receive.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    match result {
                        Ok(rules) => {
                            self.rules = rules;
                            self.loaded = true;
                        }
                        Err(error) => self.error = Some(error),
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.error = Some("A szabályművelet megszakadt.".into());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
    }
}
pub struct RulesDialog {
    root: Option<PathBuf>,
    global: Document,
    workspace: Option<Document>,
    global_tab: bool,
}
impl RulesDialog {
    pub fn open(ctx: &egui::Context, root: Option<PathBuf>) -> Self {
        let workspace = root.as_ref().map(|root| {
            let root = root.clone();
            Document::load(ctx, move || scoped_rules::workspace_path(&root))
        });
        Self {
            global_tab: root.is_none(),
            root,
            global: Document::load(ctx, scoped_rules::global_path),
            workspace,
        }
    }
    /// Returns true when closed. Root is captured at open, never taken from a switched workspace.
    pub fn show(&mut self, ctx: &egui::Context) -> bool {
        self.global.poll();
        if let Some(doc) = &mut self.workspace {
            doc.poll();
        }
        let mut close = false;
        let modal = egui::Modal::new(Id::new("scoped_rules_dialog")).show(ctx, |ui| {
            ui.set_width(560.0_f32.min(ctx.content_rect().width() - 64.0).max(240.0));
            ui.heading("Agent szabályok");
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if self.workspace.is_some() { ui.selectable_value(&mut self.global_tab, false, "Workspace"); }
                ui.selectable_value(&mut self.global_tab, true, "Globális");
            });
            ui.separator();
            let is_global = self.global_tab;
            let doc = if is_global { &mut self.global } else { self.workspace.as_mut().unwrap() };
            if doc.pending.is_some() { ui.horizontal(|ui| { ui.spinner(); ui.label("Szabályok feldolgozása…"); }); }
            if let Some(error) = &doc.error { ui.colored_label(egui::Color32::LIGHT_RED, error); }
            egui::ScrollArea::vertical().max_height((ctx.content_rect().height() - 240.0).max(160.0)).show(ui, |ui| {
                ui.add_enabled_ui(doc.loaded && doc.pending.is_none(), |ui| {
                    if !is_global {
                        ui.label(RichText::new(self.root.as_ref().unwrap().display().to_string()).small());
                        if ui.checkbox(&mut doc.rules.inherit_global, "VIBE globális szabályok öröklése").changed() { doc.saved = false; }
                        ui.label("Ütközéskor a workspace szabálya erősebb.");
                    } else {
                        ui.label("Alapértelmezett szabályok az öröklést engedélyező workspace-ekhez.");
                    }
                    ui.add_space(8.0);
                    let id = Id::new(("rules_text", is_global, &self.root));
                    let frame = crate::input_undo::prepare(ui, id, &mut doc.rules.text);
                    let response = ui.add(egui::TextEdit::multiline(&mut doc.rules.text).id(id).desired_rows(10).desired_width(f32::INFINITY).hint_text("Például: Magyarul válaszolj. Módosítás előtt olvasd el a kapcsolódó kódot.").char_limit(scoped_rules::MAX_TEXT));
                    if crate::input_undo::finish(ctx, id, frame, &doc.rules.text) || response.changed() { doc.saved = false; }
                    ui.label(RichText::new(format!("{} / 8192 bájt · Ctrl+Z: 10 lépés", doc.rules.text.len())).small());
                    if !is_global {
                        ui.add_space(10.0);
                        ui.label("Helyi szabályfájl (opcionális, relatív UTF-8 útvonal)");
                        let id = Id::new(("rules_file", &self.root));
                        let frame = crate::input_undo::prepare(ui, id, &mut doc.rules.file);
                        let response = ui.add(egui::TextEdit::singleline(&mut doc.rules.file).id(id).desired_width(f32::INFINITY).hint_text("docs/agent-rules.md").char_limit(512));
                        if crate::input_undo::finish(ctx, id, frame, &doc.rules.file) || response.changed() { doc.saved = false; }
                        ui.label(RichText::new("A fájl szabályai a fenti workspace-szöveget is felülbírálják.").small());
                    }
                    ui.add_space(12.0);
                    ui.label(RichText::new("OpenRouter: következő üzenettől. Codex / Antigravity: CLI újraindítása után. Az öröklési kapcsoló a VIBE szabályaira vonatkozik; a CLI saját AGENTS.md / GEMINI.md szabályai megmaradnak.").small());
                });
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.add_enabled(doc.loaded && doc.pending.is_none(), egui::Button::new("Szabályok mentése").min_size(vec2(160.0, 36.0))).clicked() {
                    let rules = doc.rules.clone();
                    let root = self.root.clone();
                    doc.saved = true;
                    doc.job(ctx, move || {
                        if is_global { scoped_rules::save(&scoped_rules::global_path()?, &rules)?; }
                        else { scoped_rules::save_workspace(root.as_ref().unwrap(), &rules)?; }
                        Ok(rules)
                    });
                }
                if doc.saved && doc.pending.is_none() && doc.error.is_none() { ui.label("Mentve"); }
                if ui.button("Bezárás").clicked() { close = true; }
            });
        });
        close || modal.should_close()
    }
}
