use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, LayoutMode, Panel, PanelStatus, ProviderKind, Role};

const BG: Color = Color::Rgb(10, 12, 17);
const SURFACE: Color = Color::Rgb(15, 18, 25);
const INPUT: Color = Color::Rgb(19, 23, 32);
const BORDER: Color = Color::Rgb(41, 47, 61);
const TEXT: Color = Color::Rgb(221, 226, 236);
const MUTED: Color = Color::Rgb(135, 146, 166);
const DIM: Color = Color::Rgb(92, 104, 125);
const BLUE: Color = Color::Rgb(113, 158, 249);
const ORANGE: Color = Color::Rgb(222, 158, 120);
const MINT: Color = Color::Rgb(126, 209, 181);
const PURPLE: Color = Color::Rgb(179, 154, 240);

#[derive(Debug, Default)]
pub struct UiRegions {
    pub panels: Vec<(u64, Rect)>,
    pub sidebar_panels: Vec<(u64, Rect)>,
    pub model_buttons: Vec<(u64, Rect)>,
    pub provider_buttons: Vec<(u64, Rect)>,
    pub login_button: Rect,
    pub close_buttons: Vec<(u64, Rect)>,
    pub new_panel: Rect,
    pub layout: Rect,
    pub sidebar_toggle: Rect,
}

pub fn draw(frame: &mut Frame, app: &App) -> UiRegions {
    let area = frame.area();
    let mut regions = UiRegions::default();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        area,
    );
    if area.width < 20 || area.height < 7 {
        frame.render_widget(
            Paragraph::new("VIBE\nNagyobb terminál szükséges.\nCtrl+Q · kilépés")
                .style(Style::default().fg(MUTED)),
            inset(area, 1, 1),
        );
        return regions;
    }

    let header = Rect::new(area.x, area.y, area.width, 3);
    draw_header(frame, header, app, &mut regions);
    let footer = Rect::new(area.x, area.bottom() - 1, area.width, 1);
    let mut body = Rect::new(area.x + 1, area.y + 3, area.width - 2, area.height - 4);
    if app.sidebar_visible && area.width >= 100 {
        let sidebar = Rect::new(body.x, body.y, 23, body.height);
        draw_sidebar(frame, sidebar, app, &mut regions);
        body.x += 25;
        body.width = body.width.saturating_sub(25);
    }

    let active = app.active.min(app.panels.len().saturating_sub(1));
    let tiles = panel_rects(body, app.panels.len(), app.layout);
    let compact = body.width < 62
        || body.height < 17
        || tiles.iter().any(|rect| rect.width < 28 || rect.height < 12);
    let visible: Vec<(usize, Rect)> = if app.zoomed || compact {
        app.panels
            .get(active)
            .map(|_| vec![(active, body)])
            .unwrap_or_default()
    } else {
        tiles.into_iter().enumerate().collect()
    };
    for (index, rect) in visible {
        if let Some(panel) = app.panels.get(index) {
            draw_panel(frame, rect, panel, index == active, app, &mut regions);
        }
    }
    if app.panels.is_empty() {
        frame.render_widget(
            Paragraph::new("Üres munkatér\n\nCtrl+N · új beszélgetés")
                .style(Style::default().fg(MUTED)),
            inset(body, 2, 2),
        );
    }
    let status = if compact && !app.zoomed && app.panels.len() > 1 {
        format!(" Kompakt nézet · Tab panelváltás  | {}", app.notice)
    } else if app.notice.is_empty() {
        " Tab panelváltás   Ctrl+N új panel   Ctrl+L elrendezés   F1 súgó".to_owned()
    } else {
        app.notice.clone()
    };
    frame.render_widget(
        Paragraph::new(status).style(Style::default().fg(MUTED)),
        footer,
    );

    if app.show_help {
        draw_help(frame, area);
    }
    if let Some(model) = &app.model_editor {
        draw_model_editor(frame, area, model, app);
    }
    if let Some(url) = &app.login_url {
        draw_login(frame, area, url);
    }
    regions
}

fn draw_header(frame: &mut Frame, area: Rect, app: &App, regions: &mut UiRegions) {
    let wide = area.width >= 72;
    let controls_width = if wide { 38 } else { 17 };
    let controls_x = area.right().saturating_sub(controls_width + 1);
    let left_width = controls_x.saturating_sub(area.x + 2);
    let name = if wide {
        format!("  /  {}", app.project_name)
    } else {
        String::new()
    };
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(
                "VIBE",
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
            Span::styled(name, Style::default().fg(MUTED)),
        ])),
        Rect::new(area.x + 2, area.y, left_width, 1),
    );
    let project = app.project_path.to_string_lossy();
    let project = if let Some(network) = project.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{network}")
    } else {
        project
            .strip_prefix("\\\\?\\")
            .unwrap_or(&project)
            .to_owned()
    };
    frame.render_widget(
        Paragraph::new(ellipsize(&project, left_width as usize)).style(Style::default().fg(DIM)),
        Rect::new(area.x + 2, area.y + 1, left_width, 1),
    );
    regions.sidebar_toggle = Rect::new(controls_x, area.y, 3, 2);
    frame.render_widget(
        Paragraph::new(if app.sidebar_visible { "◧" } else { "◨" })
            .style(Style::default().fg(MUTED)),
        Rect::new(controls_x + 1, area.y + 1, 1, 1),
    );
    if wide {
        regions.layout = Rect::new(controls_x + 4, area.y, 17, 2);
        frame.render_widget(
            Paragraph::new(format!(
                "{} ▾",
                if app.zoomed {
                    "Nagyított"
                } else {
                    app.layout.label()
                }
            ))
            .style(Style::default().fg(MUTED)),
            Rect::new(controls_x + 5, area.y + 1, 15, 1),
        );
    }
    regions.new_panel = Rect::new(area.right() - 14, area.y, 13, 2);
    frame.render_widget(
        Paragraph::new(" + Új panel ").style(Style::default().fg(BLUE).bg(INPUT)),
        Rect::new(regions.new_panel.x, area.y + 1, 13, 1),
    );
    if wide {
        let label = if app.auth_busy {
            "ChatGPT · kapcsolódás…".to_owned()
        } else if app.chatgpt_ready {
            format!("ChatGPT · {}", app.account_label)
        } else {
            "Continue with ChatGPT".to_owned()
        };
        regions.login_button = Rect::new(area.right() - 36, area.y, 35, 1);
        frame.render_widget(
            Paragraph::new(ellipsize(&label, 35)).style(Style::default().fg(MINT)),
            regions.login_button,
        );
    }
}

fn draw_sidebar(frame: &mut Frame, area: Rect, app: &App, regions: &mut UiRegions) {
    frame.render_widget(
        Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(BORDER)),
        area,
    );
    let x = area.x + 1;
    let width = area.width.saturating_sub(3);
    let mut lines = vec![
        Line::styled("MUNKATÉR", Style::default().fg(DIM)),
        Line::default(),
        Line::from(vec![
            Span::styled("▾ ", Style::default().fg(BLUE)),
            Span::styled(ellipsize(&app.project_name, 18), Style::default().fg(TEXT)),
        ]),
        Line::styled("  helyi projekt", Style::default().fg(DIM)),
        Line::default(),
        Line::default(),
        Line::styled("BESZÉLGETÉSEK", Style::default().fg(DIM)),
        Line::default(),
    ];
    for (index, panel) in app.panels.iter().enumerate() {
        let row_y = area.y + 1 + lines.len() as u16;
        let content_end = area.y + 1 + area.height.saturating_sub(4);
        if row_y < content_end {
            regions.sidebar_panels.push((
                panel.id,
                Rect::new(x, row_y, width, 2.min(content_end - row_y)),
            ));
        }
        let (_, color) = provider(panel);
        let selected = index == app.active;
        lines.push(
            Line::from(vec![
                Span::styled(
                    if selected { "▌ " } else { "  " },
                    Style::default().fg(BLUE),
                ),
                Span::styled("● ", Style::default().fg(color)),
                Span::styled(
                    ellipsize(&panel.title, 15),
                    Style::default().fg(if selected { TEXT } else { MUTED }),
                ),
            ])
            .style(if selected {
                Style::default().bg(INPUT)
            } else {
                Style::default()
            }),
        );
        lines.push(Line::styled(
            format!("    {}", status_label(panel.status)),
            Style::default().fg(DIM),
        ));
        lines.push(Line::default());
    }
    frame.render_widget(
        Paragraph::new(lines),
        Rect::new(x, area.y + 1, width, area.height.saturating_sub(4)),
    );
    if area.height >= 6 {
        frame.render_widget(
            Paragraph::new(if app.demo {
                "DEMÓ · helyi minták\nF1  Súgó"
            } else if app.active_panel().provider == ProviderKind::Chatgpt {
                if app.chatgpt_ready {
                    "ChatGPT · belépve\nF1  Súgó"
                } else {
                    "ChatGPT · /login\nF1  Súgó"
                }
            } else if app.active_panel().provider == ProviderKind::Antigravity {
                "Antigravity CLI · asztali app\nF1  Súgó"
            } else {
                "OpenRouter\nF1  Súgó"
            })
            .style(Style::default().fg(DIM)),
            Rect::new(x, area.bottom() - 3, width, 2),
        );
    }
}

fn draw_panel(
    frame: &mut Frame,
    area: Rect,
    panel: &Panel,
    active: bool,
    app: &App,
    regions: &mut UiRegions,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    regions.panels.push((panel.id, area));
    let border = if active { BLUE } else { BORDER };
    frame.render_widget(
        Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border))
            .style(Style::default().bg(SURFACE)),
        area,
    );
    let inner = inset(area, 1, 1);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let (name, color) = provider(panel);
    let title_width = inner.width.saturating_sub(5);
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(" ● ", Style::default().fg(color)),
            Span::styled(
                ellipsize(&panel.title, title_width.saturating_sub(3) as usize),
                Style::default().fg(TEXT).add_modifier(Modifier::BOLD),
            ),
        ])),
        Rect::new(inner.x, inner.y, title_width, 1),
    );
    if inner.width >= 8 {
        let close = Rect::new(inner.right() - 3, inner.y, 3, 1);
        regions.close_buttons.push((panel.id, close));
        frame.render_widget(Paragraph::new(" × ").style(Style::default().fg(DIM)), close);
    }
    if inner.height < 3 {
        return;
    }
    let provider_width = if inner.width >= 38 { 13 } else { 0 };
    let model_rect = Rect::new(
        inner.x + 1,
        inner.y + 1,
        inner.width.saturating_sub(2 + provider_width),
        1,
    );
    regions.model_buttons.push((panel.id, model_rect));
    let provider_label = if app.demo {
        "DEMÓ"
    } else {
        panel.provider.label()
    };
    let model = format!("{} ▾", panel.model);
    frame.render_widget(
        Paragraph::new(ellipsize(&model, model_rect.width as usize))
            .style(Style::default().fg(MUTED)),
        model_rect,
    );
    if provider_width > 0 {
        let rect = Rect::new(
            inner.right() - provider_width - 1,
            inner.y + 1,
            provider_width,
            1,
        );
        regions.provider_buttons.push((panel.id, rect));
        frame.render_widget(
            Paragraph::new(format!("{provider_label} ↔")).style(Style::default().fg(
                if panel.provider == ProviderKind::Chatgpt {
                    MINT
                } else {
                    DIM
                },
            )),
            rect,
        );
    }
    if inner.height < 7 {
        let input = Rect::new(
            inner.x + 1,
            inner.bottom() - 1,
            inner.width.saturating_sub(2),
            1,
        );
        draw_input(
            frame,
            input,
            panel,
            active && !app.show_help && app.model_editor.is_none(),
        );
        return;
    }

    let footer = Rect::new(
        inner.x + 1,
        inner.bottom() - 1,
        inner.width.saturating_sub(2),
        1,
    );
    let input = Rect::new(inner.x + 1, footer.y - 3, inner.width.saturating_sub(2), 3);
    let separator = Rect::new(inner.x, input.y - 1, inner.width, 1);
    frame.render_widget(
        Paragraph::new("─".repeat(separator.width as usize)).style(Style::default().fg(BORDER)),
        separator,
    );
    let messages = Rect::new(
        inner.x + 1,
        inner.y + 3,
        inner.width.saturating_sub(2),
        separator.y.saturating_sub(inner.y + 3),
    );
    draw_messages(frame, messages, panel, name, color, app.demo);
    draw_input(
        frame,
        input,
        panel,
        active && !app.show_help && app.model_editor.is_none(),
    );

    let status_color = match panel.status {
        PanelStatus::Running => BLUE,
        PanelStatus::Error => ORANGE,
        PanelStatus::Cancelled => MUTED,
        PanelStatus::Ready => DIM,
    };
    let mut status = status_label(panel.status).to_owned();
    if let Some(usage) = panel.usage {
        status.push_str(&format!(
            " · {} token",
            usage.prompt_tokens.saturating_add(usage.completion_tokens)
        ));
    }
    if panel.scroll > 0 {
        status.push_str(" · görgetve");
    }
    let hint = if active && footer.width >= 39 {
        "Enter küldés"
    } else {
        ""
    };
    let hint_width = hint.width() as u16;
    frame.render_widget(
        Paragraph::new(ellipsize(
            &status,
            footer.width.saturating_sub(hint_width + 1) as usize,
        ))
        .style(Style::default().fg(status_color)),
        footer,
    );
    if !hint.is_empty() {
        frame.render_widget(
            Paragraph::new(hint).style(Style::default().fg(DIM)),
            Rect::new(footer.right() - hint_width, footer.y, hint_width, 1),
        );
    }
}

fn draw_messages(
    frame: &mut Frame,
    area: Rect,
    panel: &Panel,
    provider_name: &str,
    color: Color,
    demo: bool,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let mut lines = Vec::new();
    if panel.messages.is_empty() {
        lines.push(Line::styled(
            "Saját beszélgetés. Saját kontextus.",
            Style::default().fg(MUTED),
        ));
        lines.push(Line::default());
        lines.push(Line::styled(
            "Írj üzenetet a kezdéshez.",
            Style::default().fg(DIM),
        ));
    } else {
        if demo {
            lines.push(Line::styled(
                "DEMÓ · illusztratív beszélgetés",
                Style::default().fg(DIM),
            ));
            lines.push(Line::default());
        }
        for message in &panel.messages {
            let (label, label_color) = match message.role {
                Role::User => ("TE", MUTED),
                Role::Assistant => (provider_name, color),
                Role::System => ("RENDSZER", DIM),
            };
            lines.push(Line::styled(
                label.to_owned(),
                Style::default()
                    .fg(label_color)
                    .add_modifier(Modifier::BOLD),
            ));
            for text in wrap_message(&message.content, area.width as usize) {
                let fg = if message.role == Role::System {
                    MUTED
                } else {
                    TEXT
                };
                lines.push(Line::styled(text, Style::default().fg(fg)));
            }
            lines.push(Line::default());
        }
    }
    if panel.status == PanelStatus::Running {
        lines.push(Line::styled(
            "● Válasz érkezik…",
            Style::default().fg(color),
        ));
    }
    let end = lines.len().saturating_sub(
        (panel.scroll as usize).min(lines.len().saturating_sub(area.height as usize)),
    );
    let start = end.saturating_sub(area.height as usize);
    let shown = lines[start..end].to_vec();
    let pad = area.height.saturating_sub(shown.len() as u16);
    frame.render_widget(
        Paragraph::new(shown),
        Rect::new(area.x, area.y + pad, area.width, area.height - pad),
    );
}

fn draw_input(frame: &mut Frame, area: Rect, panel: &Panel, active: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    frame.render_widget(Block::default().style(Style::default().bg(INPUT)), area);
    let prefix_width = if area.width >= 3 { 2 } else { 0 };
    let text_area = Rect::new(
        area.x + prefix_width,
        area.y,
        area.width - prefix_width,
        area.height,
    );
    if text_area.width == 0 {
        return;
    }
    if prefix_width > 0 {
        frame.render_widget(
            Paragraph::new("›").style(Style::default().fg(if active { BLUE } else { DIM })),
            Rect::new(area.x, area.y, 1, 1),
        );
    }
    let mut cursor = panel.cursor.min(panel.input.len());
    while !panel.input.is_char_boundary(cursor) {
        cursor -= 1;
    }
    let (cursor_x, cursor_y) = cursor_position(&panel.input[..cursor], text_area.width as usize);
    let mut lines = wrap_text(&panel.input, text_area.width as usize);
    while lines.len() <= cursor_y {
        lines.push(String::new());
    }
    let offset = cursor_y.saturating_sub(text_area.height as usize - 1);
    let visible: Vec<Line<'static>> = if panel.input.is_empty() {
        vec![Line::styled(
            if active {
                "Üzenet…"
            } else {
                "Üzenet ebbe a panelbe…"
            },
            Style::default().fg(DIM),
        )]
    } else {
        lines
            .into_iter()
            .skip(offset)
            .take(text_area.height as usize)
            .map(Line::raw)
            .collect()
    };
    frame.render_widget(
        Paragraph::new(visible).style(Style::default().fg(TEXT)),
        text_area,
    );
    if active {
        frame.set_cursor_position((
            text_area.x + (cursor_x as u16).min(text_area.width - 1),
            text_area.y + (cursor_y - offset) as u16,
        ));
    }
}

fn draw_help(frame: &mut Frame, area: Rect) {
    frame.render_widget(Clear, area);
    frame.render_widget(Block::default().style(Style::default().bg(BG)), area);
    let content = centered(area, 88, 37);
    let lines = vec![
        Line::styled(
            "VIBE / BILLENTYŰPARANCSOK",
            Style::default().fg(BLUE).add_modifier(Modifier::BOLD),
        ),
        Line::default(),
        Line::styled(
            "Független panelek. Külön modell, előzmény és üzenetmező.",
            Style::default().fg(MUTED),
        ),
        Line::default(),
        help_line("Tab / Shift+Tab", "Következő / előző panel"),
        help_line("Alt+1 … Alt+8", "Panel kiválasztása"),
        help_line("Ctrl+N", "Új panel"),
        help_line("Ctrl+W", "Aktív panel bezárása"),
        help_line("Ctrl+L", "Elrendezés váltása"),
        help_line("Ctrl+Z", "Aktív panel nagyítása / visszaállítás"),
        help_line("Ctrl+B", "Oldalsáv megjelenítése / elrejtése"),
        help_line("Ctrl+M", "Aktív panel modelljének módosítása"),
        help_line("Ctrl+P", "ChatGPT / OpenRouter kapcsolat váltása"),
        help_line("Ctrl+O", "Continue with ChatGPT — belépés"),
        Line::default(),
        help_line("Enter", "Üzenet küldése"),
        help_line("Alt+Enter", "Új sor az üzenetben"),
        help_line("PageUp / PageDown", "Aktív beszélgetés görgetése"),
        help_line("Ctrl+C / Esc", "Aktív válasz megszakítása"),
        help_line("Ctrl+Q", "Mentés és kilépés"),
        Line::default(),
        Line::styled("PARANCSOK", Style::default().fg(MUTED)),
        help_line(
            "/model <azonosító>",
            "Aktív kapcsolat modelljének beállítása",
        ),
        help_line(
            "/provider chatgpt",
            "Előfizetés használata az aktív panelen",
        ),
        help_line("/title <név>", "Panel átnevezése"),
        help_line("/clear", "Aktív beszélgetés törlése"),
        help_line("/login · /logout", "ChatGPT be- és kijelentkezés"),
        help_line("/models", "Előfizetéses modellista újratöltése"),
        help_line("/new · /help", "Új panel · súgó"),
        Line::default(),
        Line::styled(
            "A projektútvonal munkatér-címke; fájlok nem kerülnek automatikusan a modellhez.",
            Style::default().fg(DIM),
        ),
        Line::default(),
        Line::styled("F1 / Esc · vissza a munkatérhez", Style::default().fg(BLUE)),
    ];
    frame.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        content,
    );
}

fn help_line(key: &str, description: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<23}"), Style::default().fg(TEXT)),
        Span::styled(description.to_owned(), Style::default().fg(MUTED)),
    ])
}

fn draw_model_editor(frame: &mut Frame, area: Rect, model: &str, app: &App) {
    if app.active_panel().provider == ProviderKind::Chatgpt {
        draw_chatgpt_models(frame, area, model, app);
        return;
    }
    let dialog = centered(area, 68, 13);
    frame.render_widget(Clear, dialog);
    frame.render_widget(
        Block::default()
            .title(" Modell kiválasztása ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(BLUE))
            .style(Style::default().bg(SURFACE).fg(TEXT)),
        dialog,
    );
    let inner = inset(dialog, 2, 1);
    if inner.width == 0 || inner.height == 0 {
        return;
    }
    let lines = vec![
        Line::styled("OpenRouter modellazonosító", Style::default().fg(MUTED)),
        Line::default(),
        Line::default(),
        Line::default(),
        Line::styled(
            "Példák (elérhetőség fióktól függ):",
            Style::default().fg(DIM),
        ),
        Line::styled(
            "openrouter/auto · automatikus modellválasztás",
            Style::default().fg(BLUE),
        ),
        Line::styled("anthropic/claude-sonnet-5.5", Style::default().fg(ORANGE)),
        Line::styled(
            "openai/gpt-6.1-sol · google/gemini-3.8-flash",
            Style::default().fg(MUTED),
        ),
        Line::default(),
        Line::styled(
            "Enter · alkalmazás    Esc · mégse",
            Style::default().fg(BLUE),
        ),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
    if inner.height >= 3 {
        let input_area = Rect::new(inner.x, inner.y + 2, inner.width, 1);
        let width = input_area.width as usize;
        let mut tail = String::new();
        for ch in model.chars().rev() {
            if tail.width() + ch.width().unwrap_or(0) >= width {
                break;
            }
            tail.insert(0, ch);
        }
        frame.render_widget(
            Paragraph::new(tail.clone()).style(Style::default().bg(INPUT).fg(TEXT)),
            input_area,
        );
        frame.set_cursor_position((input_area.x + tail.width() as u16, input_area.y));
    }
}

fn draw_login(frame: &mut Frame, area: Rect, url: &str) {
    let dialog = centered(area, 100, 19);
    frame.render_widget(Clear, dialog);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Continue with ChatGPT ")
        .border_style(Style::default().fg(MINT))
        .style(Style::default().bg(SURFACE));
    let inner = block.inner(dialog);
    frame.render_widget(block, dialog);
    let content = format!(
        "Folytasd a rendszer böngészőjében.\n\nEngedélyezd a ChatGPT-csomag használatát a VIBE számára.\nJelszót és tokent ide nem kell beírnod.\n\nHa a böngésző nem nyílt meg, nyisd meg ezt a címet:\n\n{url}\n\nEsc · belépési kísérlet megszakítása"
    );
    frame.render_widget(
        Paragraph::new(content)
            .wrap(ratatui::widgets::Wrap { trim: false })
            .style(Style::default().fg(TEXT)),
        inset(inner, 1, 1),
    );
}

fn draw_chatgpt_models(frame: &mut Frame, area: Rect, model: &str, app: &App) {
    let dialog = centered(area, 84, 22);
    frame.render_widget(Clear, dialog);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" ChatGPT · előfizetéses modellek ")
        .border_style(Style::default().fg(MINT))
        .style(Style::default().bg(SURFACE));
    let inner = inset(block.inner(dialog), 1, 0);
    frame.render_widget(block, dialog);
    if inner.width == 0 || inner.height < 3 {
        return;
    }
    frame.render_widget(
        Paragraph::new("Modellazonosító (Ctrl+U: törlés)").style(Style::default().fg(MUTED)),
        Rect::new(inner.x, inner.y, inner.width, 1),
    );
    let input = Rect::new(inner.x, inner.y + 1, inner.width, 1);
    let text = ellipsize(model, inner.width.saturating_sub(1) as usize);
    frame.render_widget(
        Paragraph::new(text.clone()).style(Style::default().bg(INPUT).fg(TEXT)),
        input,
    );
    frame.set_cursor_position((
        input.x + text.width().min(input.width.saturating_sub(1) as usize) as u16,
        input.y,
    ));
    let capacity = inner.height.saturating_sub(7) as usize;
    let offset = app
        .model_selection
        .saturating_sub(capacity.saturating_sub(1));
    let mut lines = vec![
        Line::default(),
        Line::styled(
            if app.auth_busy {
                "Modellista betöltése…"
            } else if app.available_models.is_empty() {
                "/login, majd /models: modellista lekérése"
            } else {
                "Fel / Le: választás a fiók modelljei közül"
            },
            Style::default().fg(MUTED),
        ),
    ];
    for (index, choice) in app
        .available_models
        .iter()
        .enumerate()
        .skip(offset)
        .take(capacity)
    {
        lines.push(Line::styled(
            ellipsize(
                &format!(
                    "{} {}",
                    if index == app.model_selection {
                        "›"
                    } else {
                        " "
                    },
                    choice.name
                ),
                inner.width as usize,
            ),
            Style::default().fg(if index == app.model_selection {
                MINT
            } else {
                TEXT
            }),
        ));
    }
    let available = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(4),
    );
    frame.render_widget(Paragraph::new(lines), available);
    let footer = Rect::new(inner.x, inner.bottom() - 1, inner.width, 1);
    frame.render_widget(
        Paragraph::new("Enter · alkalmazás    Esc · mégse").style(Style::default().fg(MINT)),
        footer,
    );
}

fn provider(panel: &Panel) -> (&'static str, Color) {
    if panel.provider == ProviderKind::Antigravity {
        return ("ANTIGRAVITY", PURPLE);
    }
    if panel.provider == ProviderKind::Chatgpt {
        return ("OPENAI", MINT);
    }
    let model = panel.model.to_ascii_lowercase();
    if model.starts_with("anthropic/") || model.contains("claude") {
        ("CLAUDE", ORANGE)
    } else if model.starts_with("google/") || model.contains("gemini") {
        ("GEMINI", PURPLE)
    } else if model.starts_with("openai/") || model.contains("gpt") {
        ("OPENAI", MINT)
    } else {
        ("ASSZISZTENS", BLUE)
    }
}

fn status_label(status: PanelStatus) -> &'static str {
    match status {
        PanelStatus::Ready => "Készen áll",
        PanelStatus::Running => "Válasz folyamatban…",
        PanelStatus::Error => "Hiba · részletek fent",
        PanelStatus::Cancelled => "Megszakítva",
    }
}

fn panel_rects(area: Rect, count: usize, layout: LayoutMode) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    if count == 1 {
        return vec![area];
    }
    if layout == LayoutMode::Focus && count == 3 {
        let columns = partition(area, 2, true);
        let mut result = partition(columns[0], 2, false);
        result.push(columns[1]);
        return result;
    }
    if layout == LayoutMode::Columns || (layout == LayoutMode::Focus && count == 2) {
        return partition(area, count, true);
    }
    let columns = (count as f64).sqrt().ceil() as usize;
    let rows = count.div_ceil(columns);
    let mut result = Vec::with_capacity(count);
    for row in partition(area, rows, false) {
        let remaining = count - result.len();
        result.extend(partition(row, columns.min(remaining), true));
    }
    result
}

fn partition(area: Rect, count: usize, horizontal: bool) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let available = if horizontal { area.width } else { area.height } as usize;
    let gap = usize::from(available >= count.saturating_mul(2));
    let space = available.saturating_sub(gap * count.saturating_sub(1));
    let mut offset = 0;
    (0..count)
        .map(|index| {
            let size = space / count + usize::from(index < space % count);
            let rect = if horizontal {
                Rect::new(area.x + offset as u16, area.y, size as u16, area.height)
            } else {
                Rect::new(area.x, area.y + offset as u16, area.width, size as u16)
            };
            offset += size + gap;
            rect
        })
        .collect()
}

fn inset(area: Rect, x: u16, y: u16) -> Rect {
    let x = x.min(area.width / 2);
    let y = y.min(area.height / 2);
    Rect::new(
        area.x + x,
        area.y + y,
        area.width.saturating_sub(x * 2),
        area.height.saturating_sub(y * 2),
    )
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn ellipsize(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let mut result = String::new();
    let mut used = 0;
    for ch in text.chars() {
        let size = ch.width().unwrap_or(0);
        if used + size >= width {
            break;
        }
        result.push(ch);
        used += size;
    }
    result.push('…');
    result
}

fn wrap_message(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut fenced = false;
    for source in text.split('\n') {
        let is_fence = source.trim_start().starts_with("```");
        if fenced || is_fence || source.starts_with([' ', '\t']) {
            result.extend(wrap_text(source, width));
        } else {
            let mut line = String::new();
            for word in source.split_whitespace() {
                if !line.is_empty() && line.width() + 1 + word.width() > width {
                    result.push(std::mem::take(&mut line));
                }
                if !line.is_empty() {
                    line.push(' ');
                }
                if word.width() > width {
                    let mut fragments = wrap_text(word, width);
                    let last = fragments.pop().unwrap_or_default();
                    result.extend(fragments);
                    line = last;
                } else {
                    line.push_str(word);
                }
            }
            result.push(line);
        }
        if is_fence {
            fenced = !fenced;
        }
    }
    result
}

// Explicit character wrapping keeps display, history scrolling and editor cursor in agreement.
fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = vec![String::new()];
    let mut used = 0;
    for ch in text.chars() {
        if ch == '\n' {
            lines.push(String::new());
            used = 0;
            continue;
        }
        if ch == '\r' {
            continue;
        }
        let size = ch.width().unwrap_or(0);
        if used + size > width && used > 0 {
            lines.push(String::new());
            used = 0;
        }
        lines.last_mut().unwrap().push(ch);
        used += size;
    }
    lines
}

fn cursor_position(text: &str, width: usize) -> (usize, usize) {
    if width == 0 {
        return (0, 0);
    }
    let (mut x, mut y) = (0, 0);
    for ch in text.chars() {
        if ch == '\n' {
            x = 0;
            y += 1;
            continue;
        }
        if ch == '\r' {
            continue;
        }
        let size = ch.width().unwrap_or(0);
        if x + size > width {
            x = 0;
            y += 1;
        }
        x += size;
    }
    if x >= width {
        x = 0;
        y += 1;
    }
    (x, y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};
    use std::path::PathBuf;

    fn app() -> App {
        App {
            project_name: "vibe-workspace".into(),
            project_path: PathBuf::from("/workspace/project"),
            panels: [
                "anthropic/claude-sonnet-5.5",
                "openai/gpt-6.1-sol",
                "google/gemini-3.8-flash",
            ]
            .into_iter()
            .enumerate()
            .map(|(index, model)| Panel {
                attachments: Vec::new(),
                id: index as u64 + 1,
                title: ["Claude", "OpenAI", "Gemini"][index].into(),
                description: String::new(),
                group: None,
                model: model.into(),
                provider: ProviderKind::OpenRouter,
                effort: crate::app::Effort::default(),
                messages: Vec::new(),
                input: format!("Saját üzenet {}", index + 1),
                cursor: 0,
                scroll: 0,
                status: PanelStatus::Ready,
                usage: None,
                terminal_key: None,
                terminal_saved: false,
            })
            .collect(),
            active: 0,
            layout: LayoutMode::Focus,
            sidebar_visible: true,
            zoomed: false,
            show_help: false,
            model_editor: None,
            notice: String::new(),
            demo: true,
            next_id: 4,
            ..App::new(PathBuf::from("/workspace/project"), true)
        }
    }

    fn rendered_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn three_independent_inputs_and_models_are_visible() {
        let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
        let app = app();
        let mut regions = UiRegions::default();
        terminal.draw(|frame| regions = draw(frame, &app)).unwrap();
        let text = rendered_text(&terminal);
        for index in 1..=3 {
            assert!(text.contains(&format!("Saját üzenet {index}")));
        }
        assert!(text.contains("claude-sonnet"));
        assert!(text.contains("gemini-3.8-flash"));
        assert_eq!(regions.panels.len(), 3);
        assert_eq!(regions.sidebar_panels.len(), 3);
        assert_eq!(regions.model_buttons.len(), 3);
        assert_eq!(regions.close_buttons.len(), 3);
        assert!(regions.panels[2].1.height > regions.panels[0].1.height);
    }

    #[test]
    fn tiny_sizes_and_overlays_do_not_panic() {
        let mut app = app();
        for (width, height) in [
            (0, 0),
            (1, 1),
            (10, 3),
            (20, 7),
            (35, 10),
            (70, 18),
            (100, 25),
        ] {
            for overlay in 0..3 {
                app.show_help = overlay == 1;
                app.model_editor = (overlay == 2).then(|| "google/árvíztűrő-模型".into());
                let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
                terminal
                    .draw(|frame| {
                        draw(frame, &app);
                    })
                    .unwrap();
            }
        }
    }

    #[test]
    fn layouts_keep_one_to_eight_panels_inside_without_overlap() {
        let area = Rect::new(4, 3, 140, 42);
        for count in 1..=8 {
            for layout in [LayoutMode::Focus, LayoutMode::Columns, LayoutMode::Grid] {
                let rects = panel_rects(area, count, layout);
                assert_eq!(rects.len(), count);
                for (index, rect) in rects.iter().enumerate() {
                    assert_eq!(rect.intersection(area), *rect);
                    assert!(rect.width > 0 && rect.height > 0);
                    for other in rects.iter().skip(index + 1) {
                        let intersection = rect.intersection(*other);
                        assert!(intersection.width == 0 || intersection.height == 0);
                    }
                }
            }
        }
    }

    #[test]
    fn unicode_cursor_tracks_cells_and_newlines() {
        assert_eq!(cursor_position("árvíz", 20), (5, 0));
        assert_eq!(cursor_position("界a", 2), (1, 1));
        assert_eq!(cursor_position("ab\nc", 8), (1, 1));
        assert_eq!(cursor_position("abcd", 4), (0, 1));
        assert_eq!(cursor_position("abcd\ne", 4), (1, 1));
        assert_eq!(wrap_text("árvíz\n界a", 3), vec!["árv", "íz", "界a"]);
    }

    #[test]
    fn chatgpt_login_and_account_catalog_are_visible() {
        let mut app = app();
        app.demo = false;
        app.active_panel_mut().provider = ProviderKind::Chatgpt;
        app.available_models = vec![crate::app::ModelChoice {
            id: "account-model".into(),
            name: "Fiókmodell A".into(),
        }];
        app.model_editor = Some("account-model".into());
        let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
        terminal
            .draw(|frame| {
                draw(frame, &app);
            })
            .unwrap();
        assert!(rendered_text(&terminal).contains("Fiókmodell A"));
        app.model_editor = None;
        app.login_url = Some("https://auth.openai.com/api/accounts/authorize?state=test".into());
        terminal
            .draw(|frame| {
                draw(frame, &app);
            })
            .unwrap();
        assert!(rendered_text(&terminal).contains("Continue with ChatGPT"));
        assert!(rendered_text(&terminal).contains("Jelszót és tokent ide nem kell beírnod."));
    }

    #[test]
    fn eight_panels_use_compact_mode_when_tiles_cannot_fit() {
        let mut app = app();
        for id in 4..=8 {
            let mut panel = app.panels[0].clone();
            panel.id = id;
            app.panels.push(panel);
        }
        app.active = 7;
        app.notice = "DEMÓ".into();
        let mut terminal = Terminal::new(TestBackend::new(100, 25)).unwrap();
        let mut regions = UiRegions::default();
        terminal.draw(|frame| regions = draw(frame, &app)).unwrap();
        assert_eq!(regions.panels.len(), 1);
        assert_eq!(regions.panels[0].0, 8);
        assert!(rendered_text(&terminal).contains("Kompakt nézet · Tab panelváltás"));
        let mut terminal = Terminal::new(TestBackend::new(160, 48)).unwrap();
        terminal.draw(|frame| regions = draw(frame, &app)).unwrap();
        assert_eq!(regions.panels.len(), 8);
        assert_eq!(regions.sidebar_panels.len(), 8);
    }

    #[test]
    fn scrolling_reveals_history_and_clamps_at_oldest_message() {
        let mut panel = app().panels.remove(0);
        panel.messages = vec![crate::app::Message {
            role: Role::Assistant,
            content: (0..12)
                .map(|index| format!("LINE_{index:02}"))
                .collect::<Vec<_>>()
                .join("\n"),
        }];
        let mut terminal = Terminal::new(TestBackend::new(30, 5)).unwrap();
        terminal
            .draw(|frame| draw_messages(frame, frame.area(), &panel, "CLAUDE", ORANGE, false))
            .unwrap();
        let latest = rendered_text(&terminal);
        assert!(latest.contains("LINE_11"));
        assert!(!latest.contains("LINE_00"));
        panel.scroll = 4;
        terminal
            .draw(|frame| draw_messages(frame, frame.area(), &panel, "CLAUDE", ORANGE, false))
            .unwrap();
        let earlier = rendered_text(&terminal);
        assert!(earlier.contains("LINE_07"));
        assert!(!earlier.contains("LINE_11"));
        panel.scroll = u16::MAX;
        terminal
            .draw(|frame| draw_messages(frame, frame.area(), &panel, "CLAUDE", ORANGE, false))
            .unwrap();
        let oldest = rendered_text(&terminal);
        assert!(oldest.contains("CLAUDE"));
        assert!(oldest.contains("LINE_00"));
    }

    #[test]
    fn exact_wrap_then_newline_keeps_cursor_on_visible_text_row() {
        let mut panel = app().panels.remove(0);
        panel.input = "abcd\ne".into();
        panel.cursor = panel.input.len();
        let mut terminal = Terminal::new(TestBackend::new(6, 3)).unwrap();
        terminal
            .draw(|frame| draw_input(frame, frame.area(), &panel, true))
            .unwrap();
        assert_eq!(terminal.backend().buffer()[(2, 0)].symbol(), "a");
        assert_eq!(terminal.backend().buffer()[(2, 1)].symbol(), "e");
        assert_eq!(
            terminal.get_cursor_position().unwrap(),
            ratatui::layout::Position::new(3, 1)
        );
        panel.input = "abcd".into();
        panel.cursor = panel.input.len();
        terminal
            .draw(|frame| draw_input(frame, frame.area(), &panel, true))
            .unwrap();
        assert_eq!(
            terminal.get_cursor_position().unwrap(),
            ratatui::layout::Position::new(2, 1)
        );
    }
}
