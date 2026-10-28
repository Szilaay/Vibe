//! Native, cell-based view of the real Codex terminal session.
use std::time::Duration;

use eframe::egui::{self, Color32, Event, FontId, Id, Key, Modifiers, Pos2, Rect, Vec2};

use crate::terminal_backend::TerminalSession;

const BACKGROUND: Color32 = Color32::from_rgb(13, 16, 19);
const FOREGROUND: Color32 = Color32::from_rgb(225, 228, 235);
const FONT_SIZE: f32 = 14.0;

#[derive(Clone, Default)]
struct ViewState {
    instance: Option<u64>,
    undo: crate::input_undo::Undo<PromptSnapshot>,
    restored_prompt: Option<PromptSnapshot>,
    observed_prompt: Option<PromptSnapshot>,
    unseen_chars: usize,
    drag_anchor: Option<(u16, u16)>,
    read_only_selection: bool,
    selection_origin: Option<PromptSelection>,
    copied_text: Option<String>,
    selection: Option<Selection>,
    prompt_selection: Option<PromptSelection>,
    wheel_remainder: f32,
    error: Option<String>,
}

#[derive(Clone, PartialEq)]
struct PromptSnapshot {
    text: String,
    cursor: usize,
}

#[derive(Clone)]
struct PromptSelection {
    range: Selection,
    text: String,
    cursor: (u16, u16),
}

#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: (u16, u16),
    head: (u16, u16),
}

impl Selection {
    fn bounds(self) -> ((u16, u16), (u16, u16)) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    fn contains(self, row: u16, col: u16) -> bool {
        let (start, end) = self.bounds();
        (row, col) >= start && (row, col) <= end
    }
}

/// Paint only inside `rect`. The parent owns framing and panel activation.
/// Keyboard input requires both panel activation and egui keyboard focus.
pub fn show(
    ctx: &egui::Context,
    panel_id: u64,
    rect: Rect,
    terminal: &mut TerminalSession,
    active: bool,
) -> bool {
    if rect.width() < 2.0 || rect.height() < 2.0 {
        return false;
    }
    let focus_id = Id::new(("native_terminal", panel_id));
    let state_id = focus_id.with("view");
    let mut state = ctx.data_mut(|data| data.get_temp::<ViewState>(state_id).unwrap_or_default());
    if state.instance != Some(terminal.instance_id()) {
        state = ViewState {
            instance: Some(terminal.instance_id()),
            ..ViewState::default()
        };
    }
    if !terminal.is_restored()
        && state
            .error
            .as_deref()
            .is_some_and(|message| message.starts_with("Mentett CLI-nézet"))
    {
        state.error = None;
    }
    let font =
        FontId::monospace((FONT_SIZE * ctx.pixels_per_point()).round() / ctx.pixels_per_point());
    let measured_cell = ctx
        .fonts_mut(|fonts| Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font) + 2.0));
    // Render at the actual font size, with a physical-pixel cell grid. Fractional
    // Windows DPI must not place successive glyphs on different subpixel offsets.
    let (content, cell_size, size) = terminal_geometry(
        Rect::from_min_max(
            rect.min + Vec2::new(14.0, 14.0),
            rect.max - Vec2::new(18.0, 12.0),
        ),
        measured_cell,
        ctx.pixels_per_point(),
    );
    let mut screen = terminal.snapshot();
    if native_selection_menu(&screen)
        || (screen.alternate_screen() && editable_prompt(&screen).is_none())
    {
        clear_selection(&mut state);
    }
    if screen.size() != size {
        if let Err(error) = terminal.resize(size.0, size.1) {
            state.error = Some(format!("Terminal resize: {error}"));
        }
        state.selection = None;
        state.prompt_selection = None;
        screen = terminal.snapshot();
    }
    let mut requested_focus = false;
    // Middle keeps native menus, popups, and modals above terminal content.
    egui::Area::new(focus_id.with("surface"))
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .default_size(rect.size())
        .movable(false)
        .constrain(false)
        .fade_in(false)
        .sense(egui::Sense::hover())
        .show(ctx, |ui| {
            ui.set_min_size(rect.size());
            ui.set_max_size(rect.size());
            ui.set_clip_rect(rect);
            let response = ui.interact(rect, focus_id, egui::Sense::click_and_drag());
            let popup_open = egui::Popup::is_any_open(ctx);
            requested_focus =
                !popup_open && (response.clicked() || response.is_pointer_button_down_on());
            if requested_focus {
                response.request_focus();
            }
            if !active && !requested_focus && response.has_focus() {
                response.surrender_focus();
            }
            ctx.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    focus_id,
                    egui::EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
            if !popup_open && response.hovered() {
                let scroll = ctx.input_mut(|input| {
                    let mut lines = 0.0;
                    input.events.retain(|event| {
                        if let Event::MouseWheel {
                            unit,
                            delta,
                            modifiers,
                        } = event
                            && !modifiers.ctrl
                            && delta.y != 0.0
                        {
                            lines += match unit {
                                egui::MouseWheelUnit::Point => delta.y / cell_size.y,
                                egui::MouseWheelUnit::Line => delta.y * 3.0,
                                egui::MouseWheelUnit::Page => delta.y * f32::from(size.0),
                            };
                            return false;
                        }
                        true
                    });
                    lines
                });
                state.wheel_remainder += scroll;
                let lines = state.wheel_remainder.trunc() as isize;
                if lines != 0 {
                    state.wheel_remainder -= lines as f32;
                    terminal.scroll(lines);
                    state.selection = None;
                    state.prompt_selection = None;
                    screen = terminal.snapshot();
                }
            }
            if !popup_open {
                if response.drag_started()
                    && let Some(pos) = ctx.input(|input| input.pointer.press_origin())
                {
                    clear_selection(&mut state);
                    if content.contains(pos) {
                        let caret = pointer_caret(pos, content, cell_size, &screen);
                        begin_selection_drag(
                            &mut state,
                            &screen,
                            caret,
                            ctx.input(|input| input.modifiers.shift),
                        );
                    }
                }
                if (response.dragged() || response.drag_stopped())
                    && let Some(pos) = response.interact_pointer_pos()
                {
                    update_selection_drag(
                        &mut state,
                        &screen,
                        pointer_caret(pos, content, cell_size, &screen),
                    );
                }
                if response.clicked() {
                    state.selection = None;
                    state.prompt_selection = None;
                    state.selection_origin = None;
                    state.drag_anchor = None;
                    if !terminal.is_restored()
                        && let Some(pos) = response.interact_pointer_pos()
                        && content.contains(pos)
                        && let Some(bytes) = prompt_click_bytes(
                            &screen,
                            pointer_caret(pos, content, cell_size, &screen),
                        )
                        && let Err(error) = terminal.write(&bytes)
                    {
                        state.error = Some(format!("Terminal input: {error}"));
                    }
                }
            }
            // Response::has_focus reads egui's Context. Capture it before
            // input_mut takes that Context's write lock, or it deadlocks.
            let has_keyboard_focus = response.has_focus();
            if accepts_keyboard(active, has_keyboard_focus, popup_open) {
                let modifiers = ctx.input(|input| input.modifiers);
                let (writes, copy) = ctx.input_mut(|input| {
                    take_keyboard_input_with_state(
                        &mut input.events,
                        modifiers,
                        &screen,
                        active,
                        has_keyboard_focus,
                        popup_open,
                        &mut state,
                    )
                });
                if copy {
                    ctx.copy_text(
                        state
                            .copied_text
                            .take()
                            .unwrap_or_else(|| selection_text(&screen, state.selection)),
                    );
                }
                if !writes.is_empty() && terminal.is_restored() {
                    state.error =
                        Some("Mentett CLI-nézet · A fejléc ▶ gombjával folytatható.".into());
                }
                if !writes.is_empty() && !terminal.is_restored() {
                    terminal.scroll(-(screen.scrollback() as isize));
                    state.selection = None;
                    for bytes in writes {
                        if let Err(error) = terminal.write(&bytes) {
                            state.error = Some(format!("Terminal input: {error}"));
                            break;
                        }
                    }
                    screen = terminal.snapshot();
                }
            }
            let painter = ui.painter().with_clip_rect(rect);
            painter.rect_filled(rect, 12.0, BACKGROUND);
            paint_screen(
                &painter.with_clip_rect(content),
                content,
                cell_size,
                &font,
                &screen,
                state.selection,
                state.read_only_selection,
            );
            if active
                && response.has_focus()
                && !popup_open
                && screen.scrollback() == 0
                && !screen.hide_cursor()
                && !terminal.is_restored()
            {
                let time = ctx.input(|input| input.time);
                paint_cursor(
                    &painter.with_clip_rect(content),
                    content,
                    cell_size,
                    &screen,
                    time,
                );
                ctx.request_repaint_after(Duration::from_millis(250));
            }
            if screen.scrollback() > 0 {
                let jump = Rect::from_min_size(
                    Pos2::new(rect.right() - 150.0, rect.bottom() - 40.0),
                    Vec2::new(135.0, 30.0),
                );
                if ui.put(jump, egui::Button::new("↓ Legfrissebb")).clicked() {
                    terminal.scroll(-(screen.scrollback() as isize));
                    state.selection = None;
                    state.prompt_selection = None;
                }
            }
            if screen.scrollback() > 0 {
                let track = Rect::from_min_max(
                    Pos2::new(rect.right() - 7.0, content.top()),
                    Pos2::new(rect.right() - 4.0, content.bottom()),
                );
                painter.rect_filled(track, 2.0, Color32::from_rgb(32, 37, 47));
                let total = screen.scrollback_len().max(screen.scrollback()).max(1) as f32;
                let fraction = (screen.scrollback() as f32 / total).clamp(0.0, 1.0);
                let height = (track.height() * 0.15).max(16.0).min(track.height());
                let top = track.bottom() - height - fraction * (track.height() - height);
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(track.left(), top),
                        Vec2::new(track.width(), height),
                    ),
                    2.0,
                    Color32::from_rgb(87, 101, 125),
                );
            }
            if let Some(error) = state.error.as_ref() {
                let error_rect = Rect::from_min_max(
                    Pos2::new(rect.left(), rect.bottom() - cell_size.y - 6.0),
                    rect.max,
                );
                painter.rect_filled(error_rect, 0.0, Color32::from_rgb(75, 27, 30));
                painter.text(
                    error_rect.min + Vec2::new(6.0, 3.0),
                    egui::Align2::LEFT_TOP,
                    error,
                    FontId::monospace(12.0),
                    Color32::from_rgb(255, 210, 210),
                );
            }
        });
    ctx.data_mut(|data| data.insert_temp(state_id, state));
    requested_focus
}

fn accepts_keyboard(active: bool, focused: bool, popup_open: bool) -> bool {
    active && focused && !popup_open
}

#[cfg(test)]
fn take_keyboard_input(
    events: &mut Vec<Event>,
    modifiers: Modifiers,
    screen: &vt100::Screen,
    active: bool,
    focused: bool,
    popup_open: bool,
) -> (Vec<Vec<u8>>, bool) {
    take_keyboard_input_with_state(
        events,
        modifiers,
        screen,
        active,
        focused,
        popup_open,
        &mut ViewState::default(),
    )
}

fn take_keyboard_input_with_state(
    events: &mut Vec<Event>,
    modifiers: Modifiers,
    screen: &vt100::Screen,
    active: bool,
    focused: bool,
    popup_open: bool,
    view: &mut ViewState,
) -> (Vec<Vec<u8>>, bool) {
    let mut writes = Vec::new();
    let mut copy = false;
    if !accepts_keyboard(active, focused, popup_open) {
        return (writes, copy);
    }
    let observed = editable_prompt(screen).map(|prompt| prompt_snapshot(screen, &prompt));
    if observed != view.observed_prompt {
        view.observed_prompt = observed;
        view.unseen_chars = 0;
    }
    // A delayed native caret update must not cancel an otherwise unchanged
    // selection. Text/range changes do invalidate it; never then fall through
    // to a one-character Backspace against a different prompt.
    let stale_selection = view.selection.is_some()
        && view
            .prompt_selection
            .as_ref()
            .or(view.selection_origin.as_ref())
            .is_some_and(|selected| {
                screen.scrollback() != 0
                    || selection_text(screen, Some(selected.range)) != selected.text
                    || editable_prompt(screen)
                        .is_some_and(|prompt| prompt.range.bounds() != selected.range.bounds())
            });
    if stale_selection {
        view.prompt_selection = None;
        view.selection_origin = None;
        view.selection = None;
    }
    events.retain(|event| {
        let delete = matches!(event, Event::Key { key: Key::Backspace | Key::Delete, pressed: true, .. });
        if delete && (stale_selection || (view.selection.is_some() && editable_prompt(screen).is_none())) { return false; }
        if delete && view.selection.is_some() && view.read_only_selection { return false; }
        if let Event::Key { key: Key::Z, pressed: true, modifiers, .. } = event
            && (modifiers.ctrl || modifiers.mac_cmd) && !modifiers.alt && !modifiers.shift
        {
            let Some(prompt) = editable_prompt(screen) else { return false; };
            if !reconstructable(&prompt_snapshot(screen, &prompt)) {
                view.error = Some("A CLI összecsukott vagy túl hosszú bevitele nem állítható vissza a látható terminálból.".into());
                return false;
            }
            if let Some(previous) = view.undo.pop() {
                if previous.text.contains('\n') && !screen.bracketed_paste() {
                    view.undo.push(previous);
                    view.error = Some("Többsoros visszavonáshoz a CLI bracketed paste módja szükséges.".into());
                } else {
                    let mut current = view.restored_prompt.clone().unwrap_or_else(|| prompt_snapshot(screen, &prompt));
                    // Text events may share a frame with Ctrl+Z, before the PTY
                    // has echoed them. Overshooting left/delete is safe at input
                    // boundaries; never leave those queued characters behind.
                    current.text.extend(std::iter::repeat_n(' ', view.unseen_chars));
                    if !reconstructable(&current) {
                        view.undo.push(previous);
                        view.error = Some("A még feldolgozás alatt álló CLI-bevitel túl hosszú a visszavonáshoz.".into());
                        return false;
                    }
                    writes.push(restore_prompt_bytes(screen, &current, &previous));
                    view.unseen_chars = 0;
                    view.restored_prompt = Some(previous);
                    view.selection = None;
                    view.prompt_selection = None;
                }
            }
            return false;
        }
        if let Event::Key {
            key: Key::A,
            pressed: true,
            modifiers,
            ..
        } = event
            && (modifiers.ctrl || modifiers.mac_cmd)
            && !modifiers.alt
            && let Some(selected) = editable_prompt(screen)
        {
            view.selection = Some(selected.range);
            view.prompt_selection = Some(selected);
            view.selection_origin = None;
            view.read_only_selection = false;
            return false;
        }
        let text_input = matches!(event, Event::Paste(_) | Event::Text(_) | Event::Ime(egui::ImeEvent::Commit(_)))
            && input_bytes(event, modifiers, screen).is_some_and(|bytes| !bytes.is_empty());
        if text_input && view.read_only_selection { clear_selection(view); }
        let cutting = matches!(event, Event::Cut) && !modifiers.shift;
        if cutting && view.selection.is_some() && view.read_only_selection {
            view.copied_text = Some(selection_text(screen, view.selection)); copy = true; return false;
        }
        let editing = text_input || delete || cutting;
        if editing && let Some(prompt) = editable_prompt(screen) {
            if view.prompt_selection.is_none() && let Some(selected) = view.selection
                && delete_prompt_selection(screen, &prompt, selected).is_none() {
                if cutting { view.copied_text = Some(selection_text(screen, Some(selected))); copy = true; return false; }
                if delete { return false; }
            }
            view.restored_prompt = None;
            let snapshot = prompt_snapshot(screen, &prompt);
            if reconstructable(&snapshot) { view.undo.push(snapshot); } else { view.undo.clear(); }
            // Drag selections are screen ranges, not native CLI selections. Only
            // delete when the complete range lies inside the current input.
            if view.prompt_selection.is_none()
                && let Some(selected) = view.selection
            {
                if let Some(bytes) = delete_prompt_selection(screen, &prompt, selected) {
                    if cutting { view.copied_text = Some(selection_text(screen, Some(selected))); copy = true; }
                    writes.push(bytes);
                    view.selection = None;
                    view.selection_origin = None;
                    if delete || cutting { return false; }
                } else if delete { return false; }
            }
        }
        if matches!(event, Event::Key { key: Key::Enter, pressed: true, modifiers, .. } if !modifiers.shift && !modifiers.ctrl && !modifiers.alt) {
            view.undo.clear();
            view.unseen_chars = 0;
        }
        if let Some(selected) = view.prompt_selection.as_ref() {
            let delete = matches!(
                event,
                Event::Key {
                    key: Key::Backspace | Key::Delete,
                    pressed: true,
                    ..
                }
            );
            let replace = text_input;
            if delete || replace || cutting {
                // Ctrl+K is a global approval shortcut in AGY. Use only safe
                // navigation/delete sequences, never synthesized approvals.
                writes.push(clear_prompt_bytes(screen, &prompt_snapshot(screen, selected)));
                if cutting { view.copied_text = Some(selection_text(screen, view.selection)); copy = true; }
                view.prompt_selection = None;
                view.selection = None;
                if delete || cutting {
                    return false;
                }
            } else if matches!(
                event,
                Event::Key {
                    key: Key::Escape
                        | Key::ArrowLeft
                        | Key::ArrowRight
                        | Key::ArrowUp
                        | Key::ArrowDown
                        | Key::Home
                        | Key::End
                        | Key::Enter
                        | Key::Tab,
                    pressed: true,
                    ..
                }
            ) {
                view.prompt_selection = None;
                view.selection = None;
            }
        }
        // Copy stays local to the desktop terminal: forwarding ETX would make
        // both native CLIs exit after repeated Ctrl+C. Preserve Ctrl+X input.
        match event {
            Event::Copy => copy = true,
            Event::Cut if !modifiers.shift => writes.push(vec![24]),
            Event::Key {
                key: Key::C,
                pressed: true,
                modifiers,
                ..
            } if (modifiers.ctrl || modifiers.mac_cmd) && !modifiers.alt => copy = true,
            _ => {
                if let Some(bytes) = input_bytes(event, modifiers, screen) {
                    if !bytes.is_empty() {
                        if let Event::Text(text) | Event::Paste(text) | Event::Ime(egui::ImeEvent::Commit(text)) = event {
                            view.unseen_chars = view.unseen_chars.saturating_add(text.chars().count()).min(64 * 1024);
                        }
                        view.prompt_selection = None;
                        view.selection = None;
                        writes.push(bytes);
                    }
                } else {
                    return true;
                }
            }
        }
        false
    });
    (writes, copy)
}

/// Intercept editing only at a recognized visible composer containing the caret.
/// Other CLI screens keep their original keyboard shortcuts.
fn editable_prompt(screen: &vt100::Screen) -> Option<PromptSelection> {
    if screen.scrollback() != 0 || screen.hide_cursor() || native_selection_menu(screen) {
        return None;
    }
    let (rows, cols) = screen.size();
    let cursor = screen.cursor_position();
    let row_text = |row| {
        (0..cols)
            .filter_map(|col| screen.cell(row, col))
            .filter(|cell| !cell.is_wide_continuation())
            .map(|cell| {
                if cell.has_contents() {
                    cell.contents().to_owned()
                } else {
                    " ".into()
                }
            })
            .collect::<String>()
    };
    let separator = |line: &str| {
        let line = line.trim();
        line.len() > 3 && line.chars().all(|ch| "─━═╭╮╰╯┌┐└┘┬┴┼─- ".contains(ch))
    };
    let mut start = None;
    for row in (0..=cursor.0.min(rows - 1)).rev().take(64) {
        let line = row_text(row);
        if separator(&line) {
            break;
        }
        let leading = line.chars().take_while(|ch| *ch == ' ').count();
        if leading <= 4
            && ["› ", "❯ ", "> "]
                .iter()
                .any(|prefix| line.trim_start().starts_with(prefix))
        {
            let column = leading as u16 + 2;
            if row != cursor.0 || cursor.1 >= column {
                start = Some((row, column.min(cols - 1)));
            }
            break;
        }
    }
    let anchor = start?;
    let mut head = anchor;
    for row in anchor.0..rows {
        let line = row_text(row);
        if row > anchor.0
            && (separator(&line)
                || (row > cursor.0
                    && (line.trim().is_empty()
                        || line.chars().take_while(|ch| *ch == ' ').count()
                            < usize::from(anchor.1))))
        {
            break;
        }
        let first = if row == anchor.0 { anchor.1 } else { 0 };
        let end = (first..cols).rev().find(|col| {
            screen.cell(row, *col).is_some_and(|cell| {
                cell.is_wide_continuation()
                    || (cell.has_contents() && !cell.contents().trim().is_empty())
            })
        });
        if let Some(end) = end {
            head = (row, end);
        }
        if row == cursor.0 && cursor.1 > first {
            head = head.max((row, cursor.1 - 1));
        }
    }
    if cursor.0 > head.0 {
        return None;
    }
    let range = Selection { anchor, head };
    Some(PromptSelection {
        range,
        text: selection_text(screen, Some(range)),
        cursor,
    })
}

/// Move through the composer's characters using its own editing commands.
/// Counting cells would overstep wide characters and wrapped line boundaries.
fn prompt_click_bytes(screen: &vt100::Screen, target: (u16, u16)) -> Option<Vec<u8>> {
    let prompt = editable_prompt(screen)?;
    let (start, end) = prompt.range.bounds();
    if target.0 < start.0 || target.0 > end.0 {
        return None;
    }
    let cols = screen.size().1;
    let offset = |position: (u16, u16)| {
        let mut count = 0usize;
        for row in start.0..=position.0 {
            let first = if row == start.0 { start.1 } else { 0 };
            let last = if screen.row_wrapped(row) {
                cols
            } else {
                (first..cols)
                    .rev()
                    .find(|col| {
                        screen
                            .cell(row, *col)
                            .is_some_and(|cell| cell.has_contents())
                    })
                    .map_or(first, |col| col + 1)
            };
            let limit = if row == position.0 {
                position.1.clamp(first, last)
            } else {
                last
            };
            for col in first..limit {
                if let Some(cell) = screen.cell(row, col)
                    && !cell.is_wide_continuation()
                {
                    count += cell.contents().chars().count().max(1);
                }
            }
            if row < position.0 && !screen.row_wrapped(row) {
                count += 1;
            }
        }
        count
    };
    let current = offset(prompt.cursor);
    let target = offset(target);
    let key = if target < current {
        Key::ArrowLeft
    } else {
        Key::ArrowRight
    };
    let arrow = encode_key(key, Modifiers::NONE, screen.application_cursor())?;
    Some(arrow.repeat(current.abs_diff(target)))
}

fn clear_prompt_bytes(screen: &vt100::Screen, snapshot: &PromptSnapshot) -> Vec<u8> {
    let count = snapshot.text.chars().count();
    let mut bytes = encode_key(Key::ArrowLeft, Modifiers::NONE, screen.application_cursor())
        .unwrap()
        .repeat(count);
    bytes.extend(b"\x1b[3~".repeat(count));
    bytes
}

/// Logical character positions, never byte or display-column indices. Newlines
/// exist only at hard line breaks; wide continuation cells add no character.
fn prompt_offset(screen: &vt100::Screen, prompt: &PromptSelection, position: (u16, u16)) -> usize {
    let start = prompt.range.anchor;
    let end = prompt.range.head;
    let mut offset = 0;
    for row in start.0..=position.0.min(end.0) {
        let first = if row == start.0 { start.1 } else { 0 };
        let last = if row == end.0 {
            end.1.saturating_add(1)
        } else {
            screen.size().1
        };
        let limit = if row == position.0 {
            position.1.min(last)
        } else {
            last
        };
        let trimmed_end = if screen.row_wrapped(row) || row == end.0 {
            last
        } else {
            (first..last)
                .rev()
                .find(|col| {
                    screen
                        .cell(row, *col)
                        .is_some_and(|c| c.has_contents() && !c.contents().trim().is_empty())
                })
                .map_or(first, |col| col + 1)
        };
        for col in first..limit.min(trimmed_end) {
            if let Some(cell) = screen.cell(row, col)
                && !cell.is_wide_continuation()
            {
                offset += cell.contents().chars().count().max(1);
            }
        }
        if row < position.0 && !screen.row_wrapped(row) {
            offset += 1;
        }
    }
    offset
}
fn prompt_snapshot(screen: &vt100::Screen, prompt: &PromptSelection) -> PromptSnapshot {
    let (start, end) = prompt.range.bounds();
    let mut text = String::new();
    for row in start.0..=end.0 {
        let first = if row == start.0 { start.1 } else { 0 };
        let last = if row == end.0 {
            end.1 + 1
        } else {
            screen.size().1
        };
        let mut line = String::new();
        for col in first..last {
            if let Some(cell) = screen.cell(row, col)
                && !cell.is_wide_continuation()
            {
                if cell.has_contents() {
                    line.push_str(cell.contents());
                } else {
                    line.push(' ');
                }
            }
        }
        if row == end.0 || screen.row_wrapped(row) {
            text.push_str(&line);
        } else {
            text.push_str(line.trim_end());
            text.push('\n');
        }
    }
    if text.trim().is_empty() && prompt.cursor == start {
        text.clear();
    }
    PromptSnapshot {
        cursor: prompt_offset(screen, prompt, prompt.cursor).min(text.chars().count()),
        text,
    }
}
fn restore_prompt_bytes(
    screen: &vt100::Screen,
    current: &PromptSnapshot,
    previous: &PromptSnapshot,
) -> Vec<u8> {
    let mut bytes = clear_prompt_bytes(screen, current);
    bytes.extend(
        input_bytes(
            &Event::Paste(previous.text.clone()),
            Modifiers::NONE,
            screen,
        )
        .unwrap_or_default(),
    );
    let left = encode_key(Key::ArrowLeft, Modifiers::NONE, screen.application_cursor()).unwrap();
    bytes.extend(
        left.repeat(
            previous
                .text
                .chars()
                .count()
                .saturating_sub(previous.cursor),
        ),
    );
    bytes
}

fn reconstructable(snapshot: &PromptSnapshot) -> bool {
    // These labels represent private native editor data, not the underlying
    // attachment/paste text. Replacing the label would destroy that data.
    snapshot.text.len() <= 4096
        && !["[Pasted", "[Image", "[Attached"]
            .iter()
            .any(|marker| snapshot.text.contains(marker))
}
fn delete_prompt_selection(
    screen: &vt100::Screen,
    prompt: &PromptSelection,
    selection: Selection,
) -> Option<Vec<u8>> {
    let (mut start, mut end) = selection.bounds();
    let (prompt_start, prompt_end) = prompt.range.bounds();
    if start.0 < prompt_start.0 || end.0 > prompt_end.0 {
        return None;
    }
    // Dragging across the prompt marker or into the blank right margin still
    // selects the input on those rows, not read-only output or phantom spaces.
    start = start.max(prompt_start);
    end = end.min(prompt_end);
    if start > end {
        return None;
    }
    let from = prompt_offset(screen, prompt, start);
    let through = prompt_offset(screen, prompt, (end.0, end.1 + 1));
    if through <= from {
        return None;
    }
    let current = prompt_offset(screen, prompt, prompt.cursor);
    let key = if through < current {
        Key::ArrowLeft
    } else {
        Key::ArrowRight
    };
    let mut bytes = encode_key(key, Modifiers::NONE, screen.application_cursor())?
        .repeat(current.abs_diff(through));
    bytes.extend(std::iter::repeat_n(127, through - from));
    Some(bytes)
}

fn terminal_size(rect: Rect, cell: Vec2) -> (u16, u16) {
    (
        (rect.height() / cell.y.max(1.0)).floor().clamp(1.0, 1000.0) as u16,
        (rect.width() / cell.x.max(1.0)).floor().clamp(1.0, 1000.0) as u16,
    )
}

fn terminal_geometry(
    rect: Rect,
    measured_cell: Vec2,
    pixels_per_point: f32,
) -> (Rect, Vec2, (u16, u16)) {
    let scale = pixels_per_point.max(0.1);
    // Round outward so glyphs remain inside their cells; align the origin inward.
    let cell = (measured_cell * scale).ceil().max(Vec2::splat(1.0)) / scale;
    let origin = Pos2::new(
        (rect.left() * scale).ceil() / scale,
        (rect.top() * scale).ceil() / scale,
    );
    let available = Rect::from_min_max(origin, rect.max);
    let size = terminal_size(available, cell);
    let content = Rect::from_min_size(
        origin,
        cell * Vec2::new(f32::from(size.1), f32::from(size.0)),
    );
    (content, cell, size)
}

fn pointer_cell(pos: Pos2, rect: Rect, cell: Vec2, size: (u16, u16)) -> (u16, u16) {
    (
        ((pos.y - rect.top()) / cell.y)
            .floor()
            .clamp(0.0, f32::from(size.0 - 1)) as u16,
        ((pos.x - rect.left()) / cell.x)
            .floor()
            .clamp(0.0, f32::from(size.1 - 1)) as u16,
    )
}

/// Mouse selections use insertion boundaries, unlike inclusive painted cells.
/// Releasing just after a character must not also include the next character.
fn pointer_caret(pos: Pos2, rect: Rect, cell: Vec2, screen: &vt100::Screen) -> (u16, u16) {
    let (_, cols) = screen.size();
    let row = pointer_cell(pos, rect, cell, screen.size()).0;
    let x = ((pos.x - rect.left()) / cell.x).clamp(0.0, f32::from(cols));
    let mut col = x.round() as u16;
    if screen
        .cell(row, col)
        .is_some_and(|cell| cell.is_wide_continuation())
    {
        col = if x <= f32::from(col) {
            col - 1
        } else {
            col + 1
        };
    }
    (row, col)
}

fn selection_between_carets(anchor: (u16, u16), head: (u16, u16), cols: u16) -> Option<Selection> {
    if anchor == head {
        return None;
    }
    let (start, end) = if anchor < head {
        (anchor, head)
    } else {
        (head, anchor)
    };
    let end = if end.1 > 0 {
        (end.0, end.1 - 1)
    } else {
        (end.0.checked_sub(1)?, cols - 1)
    };
    Some(Selection {
        anchor: start,
        head: end,
    })
}

fn clear_selection(view: &mut ViewState) {
    view.selection = None;
    view.prompt_selection = None;
    view.selection_origin = None;
    view.drag_anchor = None;
    view.read_only_selection = false;
}

/// Resume rows also start with `›`; they are menu entries, never editable input.
fn native_selection_menu(screen: &vt100::Screen) -> bool {
    screen.rows(0, screen.size().1).take(6).any(|row| {
        let title = row.trim();
        title == "Resume a previous session" || title == "Fork a previous session"
    })
}

fn begin_selection_drag(
    view: &mut ViewState,
    screen: &vt100::Screen,
    caret: (u16, u16),
    read_only: bool,
) {
    clear_selection(view);
    if native_selection_menu(screen) {
        return;
    }
    let prompt = editable_prompt(screen);
    if read_only {
        if screen.alternate_screen() && prompt.is_none() {
            return;
        }
        let Some((first, last)) = row_text_bounds(screen, caret.0) else {
            return;
        };
        if caret.1 < first || caret.1 > last + 1 {
            return;
        }
        view.read_only_selection = true;
        view.drag_anchor = Some(caret);
    } else if let Some(prompt) = prompt {
        let (start, end) = prompt.range.bounds();
        if caret.0 < start.0 || caret.0 > end.0 || (caret.0 == end.0 && caret.1 > end.1 + 1) {
            return;
        }
        view.drag_anchor = Some(caret.max(start));
        view.selection_origin = Some(prompt);
    }
}

fn update_selection_drag(view: &mut ViewState, screen: &vt100::Screen, mut caret: (u16, u16)) {
    let Some(anchor) = view.drag_anchor else {
        return;
    };
    if native_selection_menu(screen) {
        clear_selection(view);
        return;
    }
    if !view.read_only_selection {
        let Some(origin) = &view.selection_origin else {
            clear_selection(view);
            return;
        };
        let Some(current) = editable_prompt(screen) else {
            clear_selection(view);
            return;
        };
        if current.range.bounds() != origin.range.bounds() || current.text != origin.text {
            clear_selection(view);
            return;
        }
        let (start, end) = origin.range.bounds();
        caret = caret.clamp(start, (end.0, end.1 + 1));
    }
    view.selection =
        selection_between_carets(anchor, caret, screen.size().1).and_then(|selection| {
            if !view.read_only_selection {
                return Some(selection);
            }
            // Read-only copying must not select blank trailing rows/margins.
            let (start, end) = selection.bounds();
            let mut first = None;
            let mut last = None;
            for row in start.0..=end.0 {
                if let Some((a, b)) = row_text_bounds(screen, row) {
                    let a = (row, a).max(start);
                    let b = (row, b).min(end);
                    if a <= b {
                        first.get_or_insert(a);
                        last = Some(b);
                    }
                }
            }
            Some(Selection {
                anchor: first?,
                head: last?,
            })
        });
}

fn row_text_bounds(screen: &vt100::Screen, row: u16) -> Option<(u16, u16)> {
    let mut columns = (0..screen.size().1).filter(|col| {
        screen.cell(row, *col).is_some_and(|cell| {
            cell.is_wide_continuation()
                || (cell.has_contents() && !cell.contents().trim().is_empty())
        })
    });
    let first = columns.next()?;
    Some((first, columns.next_back().unwrap_or(first)))
}

fn selection_text(screen: &vt100::Screen, selection: Option<Selection>) -> String {
    let Some(selection) = selection else {
        return screen.contents();
    };
    let (start, end) = selection.bounds();
    let mut text = String::new();
    let (_, cols) = screen.size();
    for row in start.0..=end.0 {
        let first = if row == start.0 { start.1 } else { 0 };
        let last = if row == end.0 { end.1 } else { cols - 1 };
        let mut line = String::new();
        for col in first..=last {
            if let Some(cell) = screen.cell(row, col) {
                if cell.is_wide_continuation() {
                    continue;
                }
                if cell.has_contents() {
                    line.push_str(cell.contents());
                } else {
                    line.push(' ');
                }
            }
        }
        text.push_str(line.trim_end());
        if row != end.0 && !screen.row_wrapped(row) {
            text.push('\n');
        }
    }
    text
}

fn cell_colors(cell: &vt100::Cell) -> (Color32, Color32) {
    let mut fg = terminal_color(cell.fgcolor(), FOREGROUND);
    let mut bg = terminal_color(cell.bgcolor(), BACKGROUND);
    if cell.inverse() {
        std::mem::swap(&mut fg, &mut bg);
    }
    if cell.dim() {
        fg = fg.gamma_multiply(0.6);
    }
    (fg, bg)
}

fn terminal_color(color: vt100::Color, default: Color32) -> Color32 {
    match color {
        vt100::Color::Default => default,
        vt100::Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        vt100::Color::Idx(index) => {
            const ANSI: [[u8; 3]; 16] = [
                [0, 0, 0],
                [217, 111, 124],
                [125, 196, 162],
                [220, 192, 128],
                [126, 166, 235],
                [181, 151, 215],
                [116, 191, 205],
                [229, 229, 229],
                [102, 102, 102],
                [239, 146, 155],
                [155, 218, 186],
                [238, 213, 159],
                [59, 142, 234],
                [214, 112, 214],
                [41, 184, 219],
                [255, 255, 255],
            ];
            let [r, g, b] = match index {
                0..=15 => ANSI[index as usize],
                16..=231 => {
                    let n = index - 16;
                    let component = |v| if v == 0 { 0 } else { 55 + 40 * v };
                    [component(n / 36), component((n / 6) % 6), component(n % 6)]
                }
                _ => {
                    let gray = 8 + (index - 232) * 10;
                    [gray; 3]
                }
            };
            Color32::from_rgb(r, g, b)
        }
    }
}

fn paint_screen(
    painter: &egui::Painter,
    rect: Rect,
    cell_size: Vec2,
    font: &FontId,
    screen: &vt100::Screen,
    selection: Option<Selection>,
    text_only_selection: bool,
) {
    let (rows, cols) = screen.size();
    // Paint all backgrounds first: a wide glyph's second cell must not erase it.
    for row in 0..rows {
        let selectable_columns = if text_only_selection {
            row_text_bounds(screen, row)
        } else {
            Some((0, cols - 1))
        };
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            let (_, mut bg) = cell_colors(cell);
            if selection.is_some_and(|selection| selection.contains(row, col))
                && selectable_columns.is_some_and(|(first, last)| (first..=last).contains(&col))
            {
                bg = Color32::from_rgb(56, 78, 112);
            }
            if bg != BACKGROUND {
                let pos = rect.min
                    + Vec2::new(f32::from(col) * cell_size.x, f32::from(row) * cell_size.y);
                painter.rect_filled(Rect::from_min_size(pos, cell_size), 0.0, bg);
            }
        }
    }
    for row in 0..rows {
        for col in 0..cols {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let (fg, _) = cell_colors(cell);
            let pos =
                rect.min + Vec2::new(f32::from(col) * cell_size.x, f32::from(row) * cell_size.y);
            let width = cell_size.x * if cell.is_wide() { 2.0 } else { 1.0 };
            let cell_rect = Rect::from_min_size(pos, Vec2::new(width, cell_size.y));
            let clipped = painter.with_clip_rect(painter.clip_rect().intersect(cell_rect));
            if cell.has_contents() {
                let format = egui::TextFormat {
                    font_id: if cell.bold() {
                        painter
                            .ctx()
                            .style()
                            .text_styles
                            .get(&egui::TextStyle::Name("terminal-bold".into()))
                            .map(|bold| FontId::new(font.size, bold.family.clone()))
                            .unwrap_or_else(|| font.clone())
                    } else {
                        font.clone()
                    },
                    color: fg,
                    italics: cell.italic(),
                    ..Default::default()
                };
                let galley = clipped.layout_job(egui::text::LayoutJob::single_section(
                    cell.contents().to_owned(),
                    format,
                ));
                clipped.galley(pos, galley, fg);
            }
            if cell.underline() {
                let y = pos.y + cell_size.y - 2.0;
                clipped.line_segment(
                    [Pos2::new(pos.x, y), Pos2::new(pos.x + width, y)],
                    egui::Stroke::new(1.0_f32, fg),
                );
            }
        }
    }
}

fn paint_cursor(
    painter: &egui::Painter,
    rect: Rect,
    cell: Vec2,
    screen: &vt100::Screen,
    time: f64,
) {
    use vt100::CursorStyle::*;
    let style = screen.cursor_style();
    if matches!(
        style,
        Default | BlinkBlock | BlinkUnderline | BlinkVerticalBar
    ) && !((time * 2.0) as u64).is_multiple_of(2)
    {
        return;
    }
    let (row, col) = screen.cursor_position();
    let pos = rect.min + Vec2::new(f32::from(col) * cell.x, f32::from(row) * cell.y);
    let cursor = match style {
        BlinkUnderline | SteadyUnderline => {
            Rect::from_min_size(pos + Vec2::new(0.0, cell.y - 2.0), Vec2::new(cell.x, 2.0))
        }
        BlinkVerticalBar | SteadyVerticalBar => Rect::from_min_size(pos, Vec2::new(2.0, cell.y)),
        _ => Rect::from_min_size(pos, cell),
    };
    painter.rect_filled(
        cursor,
        0.0,
        Color32::from_rgba_unmultiplied(225, 228, 235, 130),
    );
}

fn input_bytes(event: &Event, modifiers: Modifiers, screen: &vt100::Screen) -> Option<Vec<u8>> {
    match event {
        Event::Text(text) | Event::Ime(egui::ImeEvent::Commit(text)) => {
            // Ctrl+Alt is AltGr on Windows; preserve international text input.
            if (modifiers.ctrl && !modifiers.alt) || modifiers.mac_cmd {
                return Some(Vec::new());
            }
            let mut bytes = Vec::new();
            if modifiers.alt && !modifiers.ctrl {
                bytes.push(27);
            }
            bytes.extend(
                text.chars()
                    .filter(|ch| !ch.is_control())
                    .collect::<String>()
                    .as_bytes(),
            );
            Some(bytes)
        }
        Event::Paste(text) => {
            let text = text.replace("\r\n", "\n").replace('\r', "\n");
            if screen.bracketed_paste() {
                Some(format!("\x1b[200~{text}\x1b[201~").into_bytes())
            } else {
                Some(text.replace('\n', "\r").into_bytes())
            }
        }
        Event::Key {
            key,
            pressed: true,
            modifiers,
            ..
        } => encode_key(*key, *modifiers, screen.application_cursor()),
        _ => None,
    }
}

fn encode_key(key: Key, modifiers: Modifiers, application_cursor: bool) -> Option<Vec<u8>> {
    let modifier =
        1 + u8::from(modifiers.shift) + 2 * u8::from(modifiers.alt) + 4 * u8::from(modifiers.ctrl);
    let sequence = match key {
        Key::ArrowUp | Key::ArrowDown | Key::ArrowRight | Key::ArrowLeft | Key::Home | Key::End => {
            let suffix = match key {
                Key::ArrowUp => 'A',
                Key::ArrowDown => 'B',
                Key::ArrowRight => 'C',
                Key::ArrowLeft => 'D',
                Key::Home => 'H',
                _ => 'F',
            };
            if modifier != 1 {
                format!("\x1b[1;{modifier}{suffix}")
            } else if application_cursor {
                format!("\x1bO{suffix}")
            } else {
                format!("\x1b[{suffix}")
            }
        }
        Key::Insert
        | Key::Delete
        | Key::PageUp
        | Key::PageDown
        | Key::F5
        | Key::F6
        | Key::F7
        | Key::F8
        | Key::F9
        | Key::F10
        | Key::F11
        | Key::F12 => {
            let code = match key {
                Key::Insert => 2,
                Key::Delete => 3,
                Key::PageUp => 5,
                Key::PageDown => 6,
                Key::F5 => 15,
                Key::F6 => 17,
                Key::F7 => 18,
                Key::F8 => 19,
                Key::F9 => 20,
                Key::F10 => 21,
                Key::F11 => 23,
                _ => 24,
            };
            if modifier == 1 {
                format!("\x1b[{code}~")
            } else {
                format!("\x1b[{code};{modifier}~")
            }
        }
        Key::F1 | Key::F2 | Key::F3 | Key::F4 => {
            let suffix = match key {
                Key::F1 => 'P',
                Key::F2 => 'Q',
                Key::F3 => 'R',
                _ => 'S',
            };
            if modifier == 1 {
                format!("\x1bO{suffix}")
            } else {
                format!("\x1b[1;{modifier}{suffix}")
            }
        }
        Key::Tab if modifiers.shift => "\x1b[Z".to_owned(),
        Key::Tab => "\t".to_owned(),
        Key::Enter if modifiers.shift || modifiers.ctrl => format!("\x1b[13;{modifier}u"),
        Key::Enter => if modifiers.alt { "\x1b\r" } else { "\r" }.to_owned(),
        Key::Backspace => if modifiers.alt {
            "\x1b\x7f"
        } else if modifiers.ctrl {
            "\x08"
        } else {
            "\x7f"
        }
        .to_owned(),
        Key::Escape => "\x1b".to_owned(),
        _ if modifiers.ctrl && !modifiers.alt => {
            let code = match key {
                Key::A => 1,
                Key::B => 2,
                Key::C => 3,
                Key::D => 4,
                Key::E => 5,
                Key::F => 6,
                Key::G => 7,
                Key::H => 8,
                Key::I => 9,
                Key::J => 10,
                Key::K => 11,
                Key::L => 12,
                Key::M => 13,
                Key::N => 14,
                Key::O => 15,
                Key::P => 16,
                Key::Q => 17,
                Key::R => 18,
                Key::S => 19,
                Key::T => 20,
                Key::U => 21,
                Key::V => 22,
                Key::W => 23,
                Key::X => 24,
                Key::Y => 25,
                Key::Z => 26,
                Key::OpenBracket => 27,
                Key::Backslash => 28,
                Key::CloseBracket => 29,
                Key::Num6 => 30,
                Key::Minus => 31,
                Key::Space | Key::Num2 => 0,
                _ => return None,
            };
            return Some(vec![code]);
        }
        _ => return None,
    };
    Some(sequence.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercise the complete widget with the installed CLI, not only its painter.
    /// The subprocess watchdog also bounds deadlocks inside ConPTY or teardown.
    /// Credentials are synthetic; only `/` is typed, never Enter or a prompt.
    #[cfg(windows)]
    #[test]
    #[ignore = "Requires an installed interactive Codex CLI and native ConPTY"]
    fn installed_cli_full_widget_keeps_frames_responsive() {
        use std::{
            os::windows::process::CommandExt,
            process::{Command, Stdio},
            time::Instant,
        };

        const WORKER: &str = "VIBE_TERMINAL_WIDGET_SMOKE_WORKER";
        const HEARTBEAT: &str = "VIBE_TERMINAL_WIDGET_SMOKE_HEARTBEAT";
        if std::env::var_os(WORKER).is_some() {
            let heartbeat = std::path::PathBuf::from(std::env::var_os(HEARTBEAT).unwrap());
            run_installed_cli_widget_smoke(&heartbeat);
            return;
        }

        let scratch = tempfile::tempdir().unwrap();
        let heartbeat = scratch.path().join("heartbeat.txt");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "terminal_widget::tests::installed_cli_full_widget_keeps_frames_responsive",
                "--ignored",
                "--nocapture",
                "--test-threads=1",
            ])
            .env(WORKER, "1")
            .env(HEARTBEAT, &heartbeat)
            .stdin(Stdio::null())
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .unwrap();
        let started = Instant::now();
        let mut changed = Instant::now();
        let mut last = String::new();
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(
                    status.success(),
                    "Widget subprocess failed; last heartbeat: {last}"
                );
                break;
            }
            if let Ok(value) = std::fs::read_to_string(&heartbeat)
                && !value.is_empty()
                && value != last
            {
                last = value;
                changed = Instant::now();
            }
            let stall_limit = if last == "starting CLI" || last.is_empty() {
                Duration::from_secs(15)
            } else {
                Duration::from_secs(4)
            };
            if changed.elapsed() > stall_limit || started.elapsed() > Duration::from_secs(35) {
                // Kill this isolated test's complete process tree, including its CLI.
                let _ = Command::new("taskkill")
                    .args(["/PID", &child.id().to_string(), "/T", "/F"])
                    .creation_flags(0x08000000)
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                let _ = child.kill();
                let _ = child.wait();
                panic!("Widget stopped making progress; last heartbeat: {last}");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    #[cfg(windows)]
    fn run_installed_cli_widget_smoke(heartbeat: &std::path::Path) {
        use std::time::Instant;

        let report = |stage: &str| std::fs::write(heartbeat, stage).unwrap();
        report("starting CLI");
        let cwd = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let mut terminal = TerminalSession::spawn(
            cwd.path().into(),
            "gpt-6.1-sol".into(),
            crate::app::Effort::default(),
            "vibe-synthetic-widget-smoke-token-not-a-real-credential".into(),
            ctx.clone(),
        )
        .unwrap();
        let started = Instant::now();
        let mut frame = 0;
        let mut header_seen = false;
        let mut slash_sent = false;
        let mut menu_seen = false;
        // Include the narrow initial pane and rapid layout changes that a
        // backend-only test, resized after CLI startup, cannot exercise.
        let sizes = [
            Vec2::new(300.0, 300.0),
            Vec2::new(300.0, 220.0),
            Vec2::new(360.0, 260.0),
            Vec2::new(640.0, 480.0),
            Vec2::new(350.0, 620.0),
            Vec2::new(1200.0, 780.0),
        ];
        while started.elapsed() < Duration::from_secs(20) {
            let size = if frame < 80 {
                sizes[frame % sizes.len()]
            } else {
                sizes[3]
            };
            let rect = Rect::from_min_size(Pos2::ZERO, size);
            let events = if header_seen && frame >= 80 && !slash_sent {
                slash_sent = true;
                vec![Event::Text("/".into())]
            } else {
                Vec::new()
            };
            report(&format!("frame {frame}: before show"));
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(rect),
                    time: Some(started.elapsed().as_secs_f64()),
                    focused: true,
                    events,
                    ..Default::default()
                },
                |ctx| {
                    ctx.memory_mut(|memory| {
                        memory.request_focus(Id::new(("native_terminal", 910_u64)));
                    });
                    show(ctx, 910, rect, &mut terminal, true);
                },
            );
            report(&format!("frame {frame}: rendered"));
            assert!(!output.shapes.is_empty(), "Widget produced no paint output");
            let contents = terminal.snapshot().contents();
            header_seen |= contents.contains("OpenAI Codex");
            menu_seen |= contents.contains("/model");
            assert!(
                terminal.exited().is_none(),
                "CLI exited during widget smoke: {contents}"
            );
            frame += 1;
            if frame >= 120 && header_seen && menu_seen {
                break;
            }
            std::thread::sleep(Duration::from_millis(16));
        }
        assert!(header_seen, "Original Codex TUI header missing");
        assert!(
            menu_seen,
            "Widget keyboard input did not open the slash menu"
        );
        report("stopping CLI");
        terminal.stop();
        report("CLI stopped");
    }

    #[test]
    fn keyboard_routes_to_only_active_focused_panel() {
        let parser = vt100::Parser::new(3, 20, 0);
        let mut events = vec![Event::Text("prompt".into()), Event::PointerGone];
        // Drawing an inactive pane first must leave input available to its peer.
        assert!(
            take_keyboard_input(
                &mut events,
                Modifiers::NONE,
                parser.screen(),
                false,
                false,
                false
            )
            .0
            .is_empty()
        );
        assert_eq!(events.len(), 2);
        let (writes, _) = take_keyboard_input(
            &mut events,
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
        );
        assert_eq!(writes, vec![b"prompt".to_vec()]);
        assert!(matches!(events.as_slice(), [Event::PointerGone]));
        // Even a stale focus flag on the second pane cannot duplicate input.
        assert!(
            take_keyboard_input(
                &mut events,
                Modifiers::NONE,
                parser.screen(),
                false,
                true,
                false
            )
            .0
            .is_empty()
        );
        for (active, focused, popup) in [
            (true, false, false),
            (false, true, false),
            (true, true, true),
        ] {
            let mut events = vec![Event::Text("dialog text".into())];
            assert!(
                take_keyboard_input(
                    &mut events,
                    Modifiers::NONE,
                    parser.screen(),
                    active,
                    focused,
                    popup
                )
                .0
                .is_empty()
            );
            assert_eq!(events.len(), 1);
        }
    }

    #[test]
    fn clipboard_shortcuts_copy_locally_and_preserve_cut_input() {
        let parser = vt100::Parser::new(3, 20, 0);
        let mut events = vec![Event::Copy, Event::Cut];
        let (writes, copy) = take_keyboard_input(
            &mut events,
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
        );
        assert_eq!(writes, vec![vec![24]]);
        assert!(copy);
        assert!(events.is_empty());
        let mut events = vec![Event::Copy];
        let modifiers = Modifiers {
            ctrl: true,
            shift: true,
            ..Modifiers::NONE
        };
        let (writes, copy) =
            take_keyboard_input(&mut events, modifiers, parser.screen(), true, true, false);
        assert!(writes.is_empty());
        assert!(copy);
    }

    #[test]
    fn repeated_copy_never_sends_interrupt_to_either_native_cli() {
        let mut parser = vt100::Parser::new(3, 20, 0);
        parser.process(b"native CLI output");
        for modifiers in [
            Modifiers::CTRL,
            Modifiers {
                ctrl: true,
                shift: true,
                ..Modifiers::NONE
            },
        ] {
            for raw_key in [false, true] {
                let copy_event = || {
                    if raw_key {
                        Event::Key {
                            key: Key::C,
                            physical_key: None,
                            pressed: true,
                            repeat: false,
                            modifiers,
                        }
                    } else {
                        Event::Copy
                    }
                };
                let mut events = vec![copy_event(), copy_event()];
                let (writes, copy) =
                    take_keyboard_input(&mut events, modifiers, parser.screen(), true, true, false);
                assert!(writes.is_empty(), "Copy must never reach the CLI process");
                assert!(copy);
                assert!(events.is_empty());
                assert_eq!(selection_text(parser.screen(), None), "native CLI output");
                assert_eq!(
                    selection_text(
                        parser.screen(),
                        Some(Selection {
                            anchor: (0, 0),
                            head: (0, 5),
                        })
                    ),
                    "native"
                );
            }
        }
    }

    fn key_event(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn composer_fixture(message: &str) -> vt100::Parser {
        let mut parser = vt100::Parser::new(12, 60, 0);
        parser.process("Earlier response stays intact\r\n\r\n› ".as_bytes());
        parser.process(message.as_bytes());
        parser
    }

    #[test]
    fn regular_drag_is_confined_to_input_and_cannot_start_in_output_or_empty_area() {
        let parser = composer_fixture("hello world");
        let mut view = ViewState::default();
        for caret in [(0, 3), (6, 4), (2, 40)] {
            begin_selection_drag(&mut view, parser.screen(), caret, false);
            update_selection_drag(&mut view, parser.screen(), (10, 50));
            assert!(view.selection.is_none());
        }
        begin_selection_drag(&mut view, parser.screen(), (2, 2), false);
        update_selection_drag(&mut view, parser.screen(), (10, 50));
        assert_eq!(view.selection.unwrap().bounds(), ((2, 2), (2, 12)));
        assert_eq!(
            selection_text(parser.screen(), view.selection),
            "hello world"
        );
        assert!(!view.selection.unwrap().contains(3, 0));
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Backspace, Modifiers::NONE)],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert_eq!(
            apply_composer_bytes("hello world", 11, &writes.concat()),
            (String::new(), 0)
        );
    }

    #[test]
    fn resume_picker_never_gets_desktop_selection_and_ctrl_a_stays_native_archive() {
        for alternate in [false, true] {
            let mut parser = vt100::Parser::new(20, 60, 0);
            if alternate {
                parser.process(b"\x1b[?1049h");
            }
            parser.process("Resume a previous session\r\n\r\nType to search\r\n\r\n› 5h ago  Saved conversation\r\n  2d ago  Another conversation\x1b[19;1Henter resume   ctrl+a archive   esc new\x1b[5;28H".as_bytes());
            assert!(
                editable_prompt(parser.screen()).is_none(),
                "Menu rows starting with › must not be mistaken for input"
            );
            for readonly in [false, true] {
                let mut view = ViewState::default();
                begin_selection_drag(&mut view, parser.screen(), (4, 2), readonly);
                update_selection_drag(&mut view, parser.screen(), (17, 48));
                assert!(view.selection.is_none());
                let (writes, _) = take_keyboard_input_with_state(
                    &mut vec![key_event(Key::A, Modifiers::CTRL)],
                    Modifiers::CTRL,
                    parser.screen(),
                    true,
                    true,
                    false,
                    &mut view,
                );
                assert_eq!(writes, vec![vec![1]], "Archive shortcut belongs to Codex");
            }
        }
    }

    #[test]
    fn shift_drag_output_is_copy_only_and_clips_empty_rows() {
        let parser = composer_fixture("draft");
        let mut view = ViewState::default();
        begin_selection_drag(&mut view, parser.screen(), (0, 0), true);
        update_selection_drag(&mut view, parser.screen(), (10, 50));
        let selection = view.selection.unwrap();
        assert!(view.read_only_selection);
        assert_eq!(selection.bounds().1, (2, 6));
        let (writes, copied) = take_keyboard_input_with_state(
            &mut vec![Event::Copy],
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert!(writes.is_empty() && copied);
        assert!(selection_text(parser.screen(), view.selection).contains("Earlier response"));
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Backspace, Modifiers::NONE)],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert!(writes.is_empty());
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![Event::Text("x".into())],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert_eq!(
            writes,
            vec![b"x".to_vec()],
            "Typing clears output selection without deleting draft"
        );
    }

    #[test]
    fn shift_copy_does_not_paint_empty_terminal_rows_or_right_margin() {
        let ctx = egui::Context::default();
        let parser = composer_fixture("draft");
        let output = ctx.run(Default::default(), |ctx| {
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Middle,
                Id::new("copy-highlight"),
            ));
            paint_screen(
                &painter,
                Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 240.0)),
                Vec2::new(10.0, 20.0),
                &FontId::monospace(14.0),
                parser.screen(),
                Some(Selection {
                    anchor: (0, 0),
                    head: (11, 59),
                }),
                true,
            );
        });
        let blue: Vec<_> = output
            .shapes
            .iter()
            .filter_map(|shape| {
                if let egui::epaint::Shape::Rect(rect) = &shape.shape
                    && rect.fill == Color32::from_rgb(56, 78, 112)
                {
                    Some(rect.rect)
                } else {
                    None
                }
            })
            .collect();
        assert!(!blue.is_empty());
        assert!(blue.iter().all(|rect| rect.bottom() <= 60.0));
        assert!(!blue.iter().any(|rect| (20.0..40.0).contains(&rect.top())));
        assert!(blue.iter().all(|rect| rect.right() <= 290.0));
    }

    #[test]
    fn mouse_drag_uses_half_open_boundaries_in_both_directions() {
        let parser = composer_fixture("abcdef");
        let prompt = editable_prompt(parser.screen()).unwrap();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 240.0));
        let cell = Vec2::new(10.0, 20.0);
        let first = pointer_caret(Pos2::new(20.0, 50.0), rect, cell, parser.screen());
        let last = pointer_caret(Pos2::new(50.0, 50.0), rect, cell, parser.screen());
        for (a, b) in [(first, last), (last, first)] {
            let selection = selection_between_carets(a, b, 60).unwrap();
            assert_eq!(selection_text(parser.screen(), Some(selection)), "abc");
            let bytes = delete_prompt_selection(parser.screen(), &prompt, selection).unwrap();
            assert_eq!(apply_composer_bytes("abcdef", 6, &bytes), ("def".into(), 0));
        }
        assert!(selection_between_carets(first, first, 60).is_none());
    }

    #[test]
    fn selecting_whole_input_row_ignores_marker_and_right_margin() {
        for key in [Key::Backspace, Key::Delete] {
            let parser = composer_fixture("keep remove");
            let mut view = ViewState {
                selection: Some(Selection {
                    anchor: (2, 0),
                    head: (2, 59),
                }),
                ..ViewState::default()
            };
            let (writes, _) = take_keyboard_input_with_state(
                &mut vec![key_event(key, Modifiers::CTRL)],
                Modifiers::CTRL,
                parser.screen(),
                true,
                true,
                false,
                &mut view,
            );
            assert_eq!(
                apply_composer_bytes("keep remove", 11, &writes.concat()),
                (String::new(), 0)
            );
        }
    }

    #[test]
    fn wrapped_and_multiline_selection_delete_only_the_selected_characters() {
        let wrapped = "a".repeat(55) + "XYZtail";
        let parser = composer_fixture(&wrapped);
        // Column 2 + 58 characters fills the first 60-column row.
        assert!(parser.screen().row_wrapped(2));
        let selection = selection_between_carets((2, 57), (3, 2), 60).unwrap();
        let bytes = delete_prompt_selection(
            parser.screen(),
            &editable_prompt(parser.screen()).unwrap(),
            selection,
        )
        .unwrap();
        assert_eq!(
            apply_composer_bytes(&wrapped, 62, &bytes),
            ("a".repeat(55) + "il", 55)
        );
        let mut parser = composer_fixture("első sor\r\n  második sor");
        parser.process(b"\x1b[3;4H"); // caret remains before the selected range
        let selection = selection_between_carets((2, 7), (3, 5), 60).unwrap();
        let bytes = delete_prompt_selection(
            parser.screen(),
            &editable_prompt(parser.screen()).unwrap(),
            selection,
        )
        .unwrap();
        assert_eq!(
            apply_composer_bytes("első sor\n  második sor", 1, &bytes),
            ("első odik sor".into(), 5)
        );
    }

    #[test]
    fn caret_redraw_does_not_discard_ctrl_a_and_control_text_does_not_delete() {
        let mut parser = composer_fixture("hello world");
        let mut view = ViewState::default();
        take_keyboard_input_with_state(
            &mut vec![key_event(Key::A, Modifiers::CTRL)],
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![Event::Text("c".into())],
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert!(writes.is_empty());
        assert!(view.selection.is_some());
        parser.process(b"\x1b[3;6H"); // delayed cursor movement inside unchanged input
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Backspace, Modifiers::NONE)],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert_eq!(
            apply_composer_bytes("hello world", 3, &writes.concat()),
            (String::new(), 0)
        );
    }

    #[test]
    fn cut_of_local_selection_copies_exact_text_and_never_forwards_ctrl_x() {
        for full in [false, true] {
            let parser = composer_fixture("abcdef");
            let mut view = ViewState::default();
            if full {
                take_keyboard_input_with_state(
                    &mut vec![key_event(Key::A, Modifiers::CTRL)],
                    Modifiers::CTRL,
                    parser.screen(),
                    true,
                    true,
                    false,
                    &mut view,
                );
            } else {
                view.selection = selection_between_carets((2, 3), (2, 5), 60);
            }
            let (writes, copied) = take_keyboard_input_with_state(
                &mut vec![Event::Cut],
                Modifiers::CTRL,
                parser.screen(),
                true,
                true,
                false,
                &mut view,
            );
            assert!(copied);
            assert_eq!(
                view.copied_text.as_deref(),
                Some(if full { "abcdef" } else { "bc" })
            );
            assert!(!writes.concat().contains(&24));
            assert_eq!(
                apply_composer_bytes("abcdef", 6, &writes.concat()),
                if full {
                    (String::new(), 0)
                } else {
                    ("adef".into(), 1)
                }
            );
        }
    }

    #[test]
    fn wide_glyph_pointer_snaps_to_character_boundaries() {
        let parser = composer_fixture("a界b");
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(600.0, 240.0));
        let cell = Vec2::new(10.0, 20.0);
        assert_eq!(
            pointer_caret(Pos2::new(39.0, 50.0), rect, cell, parser.screen()),
            (2, 3)
        );
        assert_eq!(
            pointer_caret(Pos2::new(41.0, 50.0), rect, cell, parser.screen()),
            (2, 5)
        );
        let selection = selection_between_carets((2, 3), (2, 5), 60).unwrap();
        let bytes = delete_prompt_selection(
            parser.screen(),
            &editable_prompt(parser.screen()).unwrap(),
            selection,
        )
        .unwrap();
        assert_eq!(apply_composer_bytes("a界b", 3, &bytes), ("ab".into(), 1));
    }

    // Independent model of native editor navigation, deletion and bracketed
    // paste. Reject every control sequence capable of submit/interrupt/approval.
    fn apply_composer_bytes(text: &str, mut cursor: usize, mut bytes: &[u8]) -> (String, usize) {
        let mut chars: Vec<char> = text.chars().collect();
        while !bytes.is_empty() {
            if bytes.starts_with(b"\x1b[200~") {
                let end = bytes
                    .windows(6)
                    .position(|part| part == b"\x1b[201~")
                    .unwrap();
                let pasted: Vec<char> = std::str::from_utf8(&bytes[6..end])
                    .unwrap()
                    .chars()
                    .collect();
                let count = pasted.len();
                chars.splice(cursor..cursor, pasted);
                cursor += count;
                bytes = &bytes[end + 6..];
            } else if bytes.starts_with(b"\x1b[D") || bytes.starts_with(b"\x1bOD") {
                cursor = cursor.saturating_sub(1);
                bytes = &bytes[3..];
            } else if bytes.starts_with(b"\x1b[C") || bytes.starts_with(b"\x1bOC") {
                cursor = (cursor + 1).min(chars.len());
                bytes = &bytes[3..];
            } else if bytes.starts_with(b"\x1b[3~") {
                if cursor < chars.len() {
                    chars.remove(cursor);
                }
                bytes = &bytes[4..];
            } else if bytes[0] == 127 {
                if cursor > 0 {
                    cursor -= 1;
                    chars.remove(cursor);
                }
                bytes = &bytes[1..];
            } else {
                let ch = std::str::from_utf8(bytes).unwrap().chars().next().unwrap();
                assert!(!ch.is_control(), "Unexpected non-editing command: {ch:?}");
                chars.insert(cursor, ch);
                cursor += 1;
                bytes = &bytes[ch.len_utf8()..];
            }
        }
        (chars.into_iter().collect(), cursor)
    }

    #[test]
    fn dragging_unicode_input_deletes_whole_range_and_undo_restores_caret() {
        let mut parser = composer_fixture("árvíz界 után");
        parser.process(b"\x1b[3;6H"); // caret after three Unicode characters
        let mut view = ViewState {
            selection: Some(Selection {
                anchor: (2, 3),
                head: (2, 7),
            }),
            ..ViewState::default()
        };
        let prompt = editable_prompt(parser.screen()).unwrap();
        let original = prompt_snapshot(parser.screen(), &prompt);
        assert_eq!(original.cursor, 3);
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Backspace, Modifiers::NONE)],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        let (deleted, caret) =
            apply_composer_bytes(&original.text, original.cursor, &writes.concat());
        assert_eq!(deleted, "á után");
        assert_eq!(caret, 1);
        assert!(view.selection.is_none());
        let mut after = composer_fixture(&deleted);
        after.process(b"\x1b[3;4H");
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Z, Modifiers::CTRL)],
            Modifiers::CTRL,
            after.screen(),
            true,
            true,
            false,
            &mut view,
        );
        let restored = apply_composer_bytes(&deleted, caret, &writes.concat());
        assert_eq!(restored, (original.text, original.cursor));
    }

    #[test]
    fn output_selection_never_deletes_scrollback_and_submitting_clears_undo() {
        let parser = composer_fixture("draft");
        let mut view = ViewState {
            selection: Some(Selection {
                anchor: (0, 0),
                head: (0, 6),
            }),
            ..ViewState::default()
        };
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Backspace, Modifiers::NONE)],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert!(
            writes.is_empty(),
            "Output selection must not delete one input character"
        );
        take_keyboard_input_with_state(
            &mut vec![key_event(Key::Enter, Modifiers::NONE)],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Z, Modifiers::CTRL)],
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert!(writes.is_empty()); // no SIGTSTP and no undo into the submitted prompt
    }

    #[test]
    fn native_ctrl_z_stops_at_ten_edits() {
        let mut view = ViewState::default();
        let mut text = "a".repeat(14);
        for _ in 0..14 {
            let parser = composer_fixture(&text);
            let (writes, _) = take_keyboard_input_with_state(
                &mut vec![key_event(Key::Backspace, Modifiers::NONE)],
                Modifiers::NONE,
                parser.screen(),
                true,
                true,
                false,
                &mut view,
            );
            let (edited, _) = apply_composer_bytes(&text, text.len(), &writes.concat());
            text = edited;
        }
        for length in 1..=10 {
            let parser = composer_fixture(&text);
            let (writes, _) = take_keyboard_input_with_state(
                &mut vec![key_event(Key::Z, Modifiers::CTRL)],
                Modifiers::CTRL,
                parser.screen(),
                true,
                true,
                false,
                &mut view,
            );
            let (edited, cursor) = apply_composer_bytes(&text, text.len(), &writes.concat());
            text = edited;
            assert_eq!(text, "a".repeat(length));
            assert_eq!(cursor, length);
        }
        let parser = composer_fixture(&text);
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Z, Modifiers::CTRL)],
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert!(writes.is_empty());
    }

    #[test]
    fn undo_also_clears_text_queued_before_the_next_terminal_redraw() {
        let parser = composer_fixture("abc");
        let mut view = ViewState::default();
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![Event::Text("x".into()), key_event(Key::Z, Modifiers::CTRL)],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert_eq!(
            apply_composer_bytes("abc", 3, &writes.concat()),
            ("abc".into(), 3)
        );
        let parser = composer_fixture("[Pasted Content 200 chars]");
        view.undo.push(PromptSnapshot {
            text: "not the attachment".into(),
            cursor: 0,
        });
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Z, Modifiers::CTRL)],
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert!(writes.is_empty());
        assert!(view.error.is_some());
    }

    #[test]
    fn native_undo_restores_multiline_wrapping_and_trailing_spaces_without_submit() {
        let mut parser = composer_fixture("árvíz\r\n  második  ");
        parser.process(b"\x1b[?2004h");
        let original = prompt_snapshot(parser.screen(), &editable_prompt(parser.screen()).unwrap());
        assert_eq!(original.text, "árvíz\n  második  ");
        let current = PromptSnapshot {
            text: "other".into(),
            cursor: 2,
        };
        let bytes = restore_prompt_bytes(parser.screen(), &current, &original);
        assert_eq!(
            apply_composer_bytes(&current.text, current.cursor, &bytes),
            (original.text, original.cursor)
        );
        let parser = composer_fixture(&"a".repeat(85));
        let wrapped = prompt_snapshot(parser.screen(), &editable_prompt(parser.screen()).unwrap());
        assert_eq!(wrapped.text, "a".repeat(85));
        assert_eq!(wrapped.cursor, 85);
    }

    #[test]
    fn clicking_composer_moves_caret_and_ignores_output() {
        let parser = composer_fixture("hello world");
        assert_eq!(
            prompt_click_bytes(parser.screen(), (2, 8)),
            Some(b"\x1b[D".repeat(5))
        );
        assert_eq!(prompt_click_bytes(parser.screen(), (0, 4)), None);
        assert_eq!(
            prompt_click_bytes(parser.screen(), (2, 50)),
            Some(Vec::new())
        );
        let parser = composer_fixture("a界b");
        assert_eq!(
            prompt_click_bytes(parser.screen(), (2, 3)),
            Some(b"\x1b[D".repeat(2))
        );
    }

    #[test]
    fn ctrl_a_selects_the_composer_and_delete_clears_without_interrupt_or_submit() {
        for delete in [Key::Backspace, Key::Delete] {
            let mut parser = composer_fixture("első sor\r\n  második sor");
            // Move into the middle of the second line before selecting everything.
            parser.process(b"\x1b[4;7H");
            let mut view = ViewState::default();
            let (writes, copy) = take_keyboard_input_with_state(
                &mut vec![key_event(Key::A, Modifiers::CTRL)],
                Modifiers::CTRL,
                parser.screen(),
                true,
                true,
                false,
                &mut view,
            );
            assert!(writes.is_empty() && !copy);
            let selected = view.prompt_selection.as_ref().unwrap();
            assert!(!selected.text.contains("Earlier response"));
            assert!(selected.text.contains("első sor") && selected.text.contains("második sor"));
            let (writes, _) = take_keyboard_input_with_state(
                &mut vec![key_event(delete, Modifiers::NONE)],
                Modifiers::NONE,
                parser.screen(),
                true,
                true,
                false,
                &mut view,
            );
            let bytes = writes.concat();
            assert!(!bytes.contains(&3) && !bytes.contains(&13));
            let (message, cursor) = apply_composer_bytes("első sor\n  második sor", 12, &bytes);
            assert!(message.is_empty());
            assert_eq!(cursor, 0);
            assert!(view.prompt_selection.is_none() && view.selection.is_none());
        }
    }

    #[test]
    fn prompt_selection_supports_one_character_replacement_and_stale_selection_is_cancelled() {
        let mut parser = composer_fixture("x");
        let mut view = ViewState::default();
        take_keyboard_input_with_state(
            &mut vec![key_event(Key::A, Modifiers::CTRL)],
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert_eq!(selection_text(parser.screen(), view.selection), "x");
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![Event::Text("á@{}".into())],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert_eq!(writes.last().unwrap(), "á@{}".as_bytes());
        assert!(!writes.concat().contains(&13));
        take_keyboard_input_with_state(
            &mut vec![key_event(Key::A, Modifiers::CTRL)],
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        parser.process(b"y");
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::Backspace, Modifiers::NONE)],
            Modifiers::NONE,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert!(
            writes.is_empty(),
            "Stale selection must not turn into a one-character deletion"
        );
        assert!(view.prompt_selection.is_none());
    }

    #[test]
    fn altgr_text_reaches_cli_without_control_sequences_or_clipboard_shortcuts() {
        let parser = composer_fixture("");
        let altgr = Modifiers {
            ctrl: true,
            alt: true,
            command: true,
            ..Modifiers::NONE
        };
        let mut view = ViewState::default();
        let mut events = vec![key_event(Key::V, altgr), Event::Text("@{}[]\\|€".into())];
        let (writes, copy) = take_keyboard_input_with_state(
            &mut events,
            altgr,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert_eq!(writes.concat(), "@{}[]\\|€".as_bytes());
        assert!(!copy && view.prompt_selection.is_none());
        for (active, focused, popup) in [
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let mut view = ViewState::default();
            let mut events = vec![key_event(Key::A, Modifiers::CTRL)];
            let (writes, _) = take_keyboard_input_with_state(
                &mut events,
                Modifiers::CTRL,
                parser.screen(),
                active,
                focused,
                popup,
                &mut view,
            );
            assert!(writes.is_empty() && view.prompt_selection.is_none());
            assert_eq!(events.len(), 1);
        }
    }

    #[test]
    fn ctrl_a_keeps_native_shortcuts_outside_visible_composer() {
        let mut parser = vt100::Parser::new(8, 40, 0);
        parser.process(b"Some output, no composer");
        assert!(editable_prompt(parser.screen()).is_none());
        let mut view = ViewState::default();
        let (writes, _) = take_keyboard_input_with_state(
            &mut vec![key_event(Key::A, Modifiers::CTRL)],
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
            &mut view,
        );
        assert_eq!(writes, vec![vec![1]]);
        let mut parser = composer_fixture("draft");
        parser.process(b"\x1b[?25l");
        assert!(editable_prompt(parser.screen()).is_none());
    }

    #[test]
    fn keys_preserve_terminal_sequences_and_modifiers() {
        assert_eq!(encode_key(Key::C, Modifiers::CTRL, false), Some(vec![3]));
        assert_eq!(encode_key(Key::D, Modifiers::CTRL, false), Some(vec![4]));
        assert_eq!(
            encode_key(Key::ArrowUp, Modifiers::NONE, true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            encode_key(Key::ArrowLeft, Modifiers::CTRL, false),
            Some(b"\x1b[1;5D".to_vec())
        );
        assert_eq!(
            encode_key(Key::Tab, Modifiers::SHIFT, false),
            Some(b"\x1b[Z".to_vec())
        );
        assert_eq!(
            encode_key(Key::Enter, Modifiers::SHIFT, false),
            Some(b"\x1b[13;2u".to_vec())
        );
        assert_eq!(
            encode_key(Key::F12, Modifiers::NONE, false),
            Some(b"\x1b[24~".to_vec())
        );
        assert!(encode_key(Key::A, Modifiers::NONE, false).is_none());
    }

    #[test]
    fn image_paste_shortcut_reaches_only_the_focused_terminal_once() {
        let mut parser = vt100::Parser::new(3, 30, 0);
        parser.process(b"\x1b[?2004h");
        let shortcut = Event::Key {
            key: Key::V,
            physical_key: Some(Key::V),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::CTRL,
        };
        for (active, focused, popup) in [
            (false, true, false),
            (true, false, false),
            (true, true, true),
        ] {
            let mut events = vec![shortcut.clone()];
            let (writes, _) = take_keyboard_input(
                &mut events,
                Modifiers::CTRL,
                parser.screen(),
                active,
                focused,
                popup,
            );
            assert!(writes.is_empty());
            assert_eq!(events.len(), 1);
        }
        let mut events = vec![
            shortcut,
            Event::Key {
                key: Key::V,
                physical_key: Some(Key::V),
                pressed: false,
                repeat: false,
                modifiers: Modifiers::CTRL,
            },
        ];
        let (writes, copy) = take_keyboard_input(
            &mut events,
            Modifiers::CTRL,
            parser.screen(),
            true,
            true,
            false,
        );
        // Raw Ctrl+V invokes Codex's native image attachment, never a bracketed
        // text paste and never Enter (which would submit the prompt).
        assert_eq!(writes, vec![vec![0x16]]);
        assert!(!copy);
    }

    #[test]
    fn paste_and_international_text_keep_original_content() {
        let mut parser = vt100::Parser::new(3, 30, 10);
        parser.process(b"\x1b[?2004h");
        assert_eq!(
            input_bytes(
                &Event::Paste("a\r\nb".into()),
                Modifiers::CTRL,
                parser.screen()
            ),
            Some(b"\x1b[200~a\nb\x1b[201~".to_vec())
        );
        let altgr = Modifiers {
            ctrl: true,
            alt: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            input_bytes(&Event::Text("árvíz界".into()), altgr, parser.screen()),
            Some("árvíz界".as_bytes().to_vec())
        );
        assert_eq!(
            input_bytes(&Event::Text("c".into()), Modifiers::CTRL, parser.screen()),
            Some(Vec::new())
        );
    }

    #[test]
    fn ansi_colors_inverse_and_wide_selection_survive() {
        let mut parser = vt100::Parser::new(3, 20, 10);
        parser.process("\x1b[1;4;38;2;12;34;56;48;5;196mA界\x1b[7mB".as_bytes());
        let screen = parser.screen();
        let first = screen.cell(0, 0).unwrap();
        assert!(first.bold() && first.underline());
        assert_eq!(
            cell_colors(first),
            (Color32::from_rgb(12, 34, 56), Color32::RED)
        );
        assert!(screen.cell(0, 1).unwrap().is_wide());
        assert!(screen.cell(0, 2).unwrap().is_wide_continuation());
        assert_eq!(
            cell_colors(screen.cell(0, 3).unwrap()),
            (Color32::RED, Color32::from_rgb(12, 34, 56))
        );
        assert_eq!(
            selection_text(
                screen,
                Some(Selection {
                    anchor: (0, 0),
                    head: (0, 3)
                })
            ),
            "A界B"
        );
        assert_eq!(
            terminal_color(vt100::Color::Idx(232), BACKGROUND),
            Color32::from_gray(8)
        );
    }

    #[test]
    fn terminal_grid_stays_on_physical_pixels_at_fractional_dpi() {
        let rect = Rect::from_min_size(Pos2::new(20.3, 30.7), Vec2::new(640.0, 480.0));
        let measured = Vec2::new(8.4, 16.3);
        for scale in [1.0, 1.25, 1.5, 1.75, 2.0] {
            let (content, cell, (rows, cols)) = terminal_geometry(rect, measured, scale);
            assert!(rect.contains_rect(content));
            assert!(cell.x >= measured.x && cell.y >= measured.y);
            for value in [content.left(), content.top(), cell.x, cell.y] {
                let physical = value * scale;
                assert!((physical - physical.round()).abs() < 0.001);
            }
            let cursor = content.min + cell * Vec2::new(f32::from(cols - 1), f32::from(rows - 1));
            assert!((cursor.x * scale - (cursor.x * scale).round()).abs() < 0.001);
            assert!((cursor.y * scale - (cursor.y * scale).round()).abs() < 0.001);
            assert_eq!(
                pointer_cell(cursor + cell * 0.5, content, cell, (rows, cols)),
                (rows - 1, cols - 1)
            );
        }
    }

    #[test]
    fn bold_terminal_glyph_is_drawn_once_without_offset_overpainting() {
        let ctx = egui::Context::default();
        crate::desktop_ui::configure(&ctx);
        let mut parser = vt100::Parser::new(1, 4, 0);
        parser.process(b"\x1b[1mA");
        let output = ctx.run(Default::default(), |ctx| {
            let font = FontId::monospace(FONT_SIZE);
            let painter = ctx.layer_painter(egui::LayerId::new(
                egui::Order::Middle,
                Id::new("bold-test"),
            ));
            paint_screen(
                &painter,
                Rect::from_min_size(Pos2::ZERO, Vec2::new(200.0, 40.0)),
                Vec2::new(12.0, 22.0),
                &font,
                parser.screen(),
                None,
                false,
            );
        });
        let texts = output
            .shapes
            .iter()
            .filter(|shape| matches!(shape.shape, egui::epaint::Shape::Text(_)))
            .count();
        assert_eq!(
            texts, 1,
            "bold must use one real face, never a second shifted glyph"
        );
    }

    #[test]
    fn rendering_stays_clipped_and_uses_measured_cell_geometry() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(Pos2::new(20.0, 30.0), Vec2::new(320.0, 160.0));
        let mut parser = vt100::Parser::new(8, 30, 0);
        parser.process("\x1b[44;1;4mCodex 界\x1b[0m\r\nready".as_bytes());
        let output = ctx.run(Default::default(), |ctx| {
            let font = FontId::monospace(FONT_SIZE);
            let cell = ctx.fonts_mut(|fonts| {
                Vec2::new(fonts.glyph_width(&font, 'M'), fonts.row_height(&font))
            });
            assert!(cell.x > 0.0 && cell.y > cell.x);
            assert_eq!(
                terminal_size(Rect::from_min_size(Pos2::ZERO, cell * 10.0), cell),
                (10, 10)
            );
            let painter = ctx
                .layer_painter(egui::LayerId::new(egui::Order::Middle, Id::new("test")))
                .with_clip_rect(rect);
            paint_screen(&painter, rect, cell, &font, parser.screen(), None, false);
        });
        assert!(output.shapes.len() > 10);
        assert!(
            output
                .shapes
                .iter()
                .all(|shape| rect.contains_rect(shape.clip_rect))
        );
    }
}
