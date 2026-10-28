//! Offline screenshot harness. No controller, account access, CLI process, or network.
use eframe::egui;
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use vibe::{
    app::{App, Effort, ProviderKind},
    desktop_ui::{self, DesktopView},
    terminal_backend::TerminalSession,
    terminal_persistence::{TerminalArchive, TerminalStore},
};
struct Preview {
    app: App,
    view: DesktopView,
    terminals: HashMap<u64, TerminalSession>,
    frames: usize,
    output: PathBuf,
    usage: bool,
    google_usage: bool,
    group_menu: bool,
    move_menu: bool,
}
impl eframe::App for Preview {
    fn raw_input_hook(&mut self, _: &egui::Context, input: &mut egui::RawInput) {
        if (self.usage || self.google_usage || self.group_menu || self.move_menu)
            && matches!(self.frames, 3 | 4)
        {
            let pos = if self.group_menu {
                egui::pos2(140.0, 334.0)
            } else if self.move_menu {
                egui::pos2(140.0, 254.0)
            } else if self.google_usage {
                egui::pos2(1180.0, 40.0)
            } else {
                egui::pos2(1040.0, 40.0)
            };
            input.events.push(egui::Event::PointerMoved(pos));
            input.events.push(egui::Event::PointerButton {
                pos,
                button: if self.group_menu || self.move_menu {
                    egui::PointerButton::Secondary
                } else {
                    egui::PointerButton::Primary
                },
                pressed: self.frames == 3,
                modifiers: Default::default(),
            });
        }
    }
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        let screenshots = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| {
                    if let egui::Event::Screenshot { image, .. } = event {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        });
        for image in screenshots {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&(image.width() as u32).to_le_bytes());
            bytes.extend_from_slice(&(image.height() as u32).to_le_bytes());
            for pixel in &image.pixels {
                bytes.extend_from_slice(&pixel.to_array());
            }
            std::fs::write(&self.output, bytes).unwrap();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let _ = desktop_ui::draw(ctx, &mut self.app, &mut self.view, false);
        for (id, rect) in self.view.terminal_regions() {
            let active = self.app.active_panel().id == id;
            if let Some(terminal) = self.terminals.get_mut(&id) {
                vibe::terminal_widget::show(ctx, id, rect, terminal, active);
            }
        }
        self.frames += 1;
        if self.frames == 24 {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }
}
fn main() -> eframe::Result {
    let output = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .expect("output .rgba path");
    let usage = std::env::args().any(|arg| arg == "--usage");
    let google_usage = std::env::args().any(|arg| arg == "--google-usage");
    let group_menu = std::env::args().any(|arg| arg == "--group-menu");
    let move_menu = std::env::args().any(|arg| arg == "--move-menu");
    eframe::run_native(
        "Offline design preview",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1600.0, 1000.0])
                .with_decorations(false)
                .with_visible(false),
            ..Default::default()
        },
        Box::new(move |cc| {
            desktop_ui::configure(&cc.egui_ctx);
            let mut app = App::new(PathBuf::from("VibeCoding App"), true);
            app.demo = false;
            app.chatgpt_ready = true;
            app.antigravity_enabled = true;
            app.account_label = "Szili".into();
            // Synthetic sample data only; never fetched from an account.
            app.native_codex_account = Some(vibe::codex_account::AccountInfo {
                email: Some("szili@example.test".into()),
                plan_type: Some("plus".into()),
            });
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            app.account_usage.snapshot = Some(vibe::account_usage::AccountUsage {
                primary: Some(vibe::account_usage::UsageWindow {
                    remaining_percent: 72.0,
                    window_seconds: Some(18000),
                    resets_at: Some(now + 7200),
                }),
                secondary: Some(vibe::account_usage::UsageWindow {
                    remaining_percent: 86.0,
                    window_seconds: Some(604800),
                    resets_at: Some(now + 86400),
                }),
                fetched_at: now - 180,
                plan_type: Some("plus".into()),
            });
            app.antigravity_usage.snapshot = Some(vibe::antigravity_usage::AntigravityUsage {
                models: vec![vibe::antigravity_usage::ModelQuota {
                    name: "Gemini".into(),
                    remaining_percent: Some(86.0),
                    remaining_amount: None,
                    reset_at: None,
                    reset_label: None,
                }],
                fetched_at: now - 180,
                account_label: None,
                plan_label: None,
            });
            if let Some(usage) = app.antigravity_usage.snapshot.as_mut() {
                for (name, percent) in [("Gemini Pro", 74.0), ("Gemini Flash", 91.0)] {
                    usage.models.push(vibe::antigravity_usage::ModelQuota {
                        name: name.into(),
                        remaining_percent: Some(percent),
                        remaining_amount: None,
                        reset_at: Some(now + 7200),
                        reset_label: None,
                    });
                }
            }
            if let Some(project) = app.recent_projects.first_mut() {
                project.pinned = true;
            }

            app.panels.truncate(2);
            app.layout = vibe::app::LayoutMode::Columns;
            let mut terminals = HashMap::new();
            for (index, panel) in app.panels.iter_mut().enumerate() {
                panel.title = if index == 0 { "Sophie" } else { "Rose" }.into();
                panel.provider = if index == 0 {
                    ProviderKind::Chatgpt
                } else {
                    ProviderKind::Antigravity
                };
                panel.model = if index == 0 { "gpt-6.1-sol" } else { "gemini" }.into();
                panel.terminal_saved = true;
                let mut parser = vt100::Parser::new(35, 70, 1000);
                let text = if index == 0 {
                    "OFFLINE DESIGN PREVIEW\r\n\r\n\x1b[32m✓\x1b[0m Analyzed the codebase\r\n\r\nUpdated src/app.rs\r\n\r\n\x1b[32m+ pub struct AppState {\r\n+     pub workspace: String,\r\n+ }\x1b[0m\r\n\r\nAll tests passed.\r\n\r\n› Ask Codex to do anything"
                } else {
                    "OFFLINE DESIGN PREVIEW\r\n\r\nPlanning implementation...\r\n\r\n1. Review workspace configuration\r\n2. Update the application layout\r\n3. Verify keyboard and menu behavior\r\n\r\nReady for your next task.\r\n\r\n› Send a message"
                };
                parser.process(text.as_bytes());
                let archive = TerminalArchive::capture(parser.screen(), panel.provider);
                let store = Arc::new(
                    TerminalStore::open(&uuid::Uuid::new_v4().to_string(), panel.provider).unwrap(),
                );
                terminals.insert(
                    panel.id,
                    TerminalSession::restored(
                        archive,
                        store,
                        panel.model.clone(),
                        Effort::Fast,
                        cc.egui_ctx.clone(),
                    )
                    .unwrap(),
                );
            }
            app.workspace_groups.push("Frontend".into());
            for index in 0..2 {
                let mut panel = app.panels[index].clone();
                panel.id += 100;
                panel.group = Some("Frontend".into());
                app.panels.push(panel);
            }
            if std::env::args().any(|arg| {
                matches!(
                    arg.as_str(),
                    "--openrouter"
                        | "--edit-review"
                        | "--files"
                        | "--changes"
                        | "--file-preview"
                        | "--diff-preview"
                )
            }) {
                use vibe::app::{Message, PanelStatus, Role};
                terminals.clear();
                app.panels.truncate(1);
                let panel = &mut app.panels[0];
                panel.provider = ProviderKind::OpenRouter;
                panel.title = "VIBE Agent".into();
                panel.model = "example/tool-capable-model".into();
                panel.terminal_saved = false;
                panel.status = PanelStatus::Ready;
                panel.messages = vec![
                    Message {role:Role::System,content:"OFFLINE PREVIEW · szintetikus adatok, nincs modellhívás vagy fájlmódosítás.".into()},
                    Message {role:Role::User,content:"Nézd át a projektet, és javítsd a gomb megjelenését.".into()},
                    Message {role:Role::System,content:"OK list_directory\n[dir] src\n      Cargo.toml\n      README.md".into()},
                    Message {role:Role::System,content:"OK read_file\n1 | fn button_label() -> &'static str {\n2 |     \"Send\"\n3 | }".into()},
                    Message {role:Role::Assistant,content:"Megtaláltam a gomb címkéjét a src/button.rs fájlban.\n\nA javaslat rövidebb feliratot használ; a módosítást jóváhagyás után menti az ügynök.\n\nParancsot vagy tesztet ebben az előnézetben nem futtattam.".into()},
                ];
                panel.input = "Mutasd a következő módosítást…".into();
                if std::env::args().any(|arg| arg == "--edit-review") {
                    app.edit_reviews.insert(
                        panel.id,
                        vibe::workspace_tools::EditProposal {
                            path: "src/button.rs".into(),
                            before: Some(
                                "fn button_label() -> &'static str {\n    \"Send\"\n}\n".into(),
                            ),
                            after: "fn button_label() -> &'static str {\n    \"Run\"\n}\n".into(),
                        },
                    );
                }
            }
            let mut view = DesktopView::default();
            if std::env::args().any(|arg| arg == "--router-compare") {
                app.panels.truncate(2);
                app.workspace_groups.clear();
                app.active_group = None;
                app.layout = vibe::app::LayoutMode::Columns;
                app.active = 1;
                for panel in &mut app.panels {
                    panel.group = None;
                }
                let panel = &mut app.panels[1];
                terminals.remove(&panel.id);
                panel.provider = ProviderKind::OpenRouter;
                panel.title = "OpenRouter".into();
                panel.model = "openrouter/auto".into();
                panel.terminal_saved = false;
                panel.messages.clear();
                panel.input.clear();
                panel.status = vibe::app::PanelStatus::Ready;
            }
            let file_mode = std::env::args().any(|arg| {
                matches!(
                    arg.as_str(),
                    "--files" | "--changes" | "--file-preview" | "--diff-preview"
                )
            });
            if file_mode {
                use vibe::project_files::{Changes, GitChange, Load, Request};
                use vibe::workspace_tools::{Directory, FileEntry};
                app.files.directories.insert(
                    ".".into(),
                    Load::Ready(Directory {
                        truncated: false,
                        entries: vec![
                            FileEntry {
                                name: "src".into(),
                                path: "src".into(),
                                directory: true,
                            },
                            FileEntry {
                                name: "Cargo.toml".into(),
                                path: "Cargo.toml".into(),
                                directory: false,
                            },
                            FileEntry {
                                name: "README.md".into(),
                                path: "README.md".into(),
                                directory: false,
                            },
                        ],
                    }),
                );
                app.files.directories.insert(
                    "src".into(),
                    Load::Ready(Directory {
                        truncated: false,
                        entries: vec![FileEntry {
                            name: "button.rs".into(),
                            path: "src/button.rs".into(),
                            directory: false,
                        }],
                    }),
                );
                app.files.changes = Some(Load::Ready(Changes {
                    truncated: false,
                    entries: vec![
                        GitChange {
                            path: "src/button.rs".into(),
                            status: " M".into(),
                        },
                        GitChange {
                            path: "README.md".into(),
                            status: "M ".into(),
                        },
                        GitChange {
                            path: "src/theme.rs".into(),
                            status: "??".into(),
                        },
                    ],
                }));
                app.panels[0].attachments = vec!["README.md".into()];
                view.open_files();
                if std::env::args().any(|arg| arg == "--changes" || arg == "--diff-preview") {
                    view.open_changes();
                }
                if std::env::args().any(|arg| arg == "--file-preview") {
                    app.files.preview = Some((
                        Request::File("src/button.rs".into()),
                        Load::Ready("fn button_label() -> &'static str {\n    \"Run\"\n}\n".into()),
                    ));
                }
                if std::env::args().any(|arg| arg == "--diff-preview") {
                    app.files.preview=Some((Request::Diff("src/button.rs".into()),Load::Ready("# WORKING TREE\ndiff --git a/src/button.rs b/src/button.rs\n--- a/src/button.rs\n+++ b/src/button.rs\n@@ -1,3 +1,3 @@\n fn button_label() -> &'static str {\n-    \"Send\"\n+    \"Run\"\n }\n".into())));
                }
            }
            if std::env::args().any(|arg| arg == "--welcome") {
                app = App::welcome();
                terminals.clear();
                view = DesktopView::launcher();
            }
            if std::env::args().any(|arg| arg == "--rules") {
                view.open_rules(&cc.egui_ctx, Some(std::env::current_dir().unwrap()));
            }
            Ok(Box::new(Preview {
                app,
                view,
                terminals,
                frames: 0,
                output,
                usage,
                google_usage,
                group_menu,
                move_menu,
            }))
        }),
    )
}
