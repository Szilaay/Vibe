//! Bounded, per-editor history. Cursor indices are Unicode character indices.
use eframe::egui::{
    self, Id, Key,
    text::{CCursor, CCursorRange},
};
use std::collections::VecDeque;

#[derive(Clone, Debug)]
pub struct Undo<T> {
    entries: VecDeque<T>,
}
impl<T> Default for Undo<T> {
    fn default() -> Self {
        Self {
            entries: VecDeque::new(),
        }
    }
}
impl<T: PartialEq> Undo<T> {
    pub fn push(&mut self, state: T) {
        if self.entries.back() != Some(&state) {
            if self.entries.len() == 10 {
                self.entries.pop_front();
            }
            self.entries.push_back(state);
        }
    }
    pub fn pop(&mut self) -> Option<T> {
        self.entries.pop_back()
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}
#[derive(Clone, PartialEq)]
struct Snapshot {
    text: String,
    cursor: CCursorRange,
}
#[derive(Clone, Default)]
struct Editor {
    undo: Undo<Snapshot>,
    expected: String,
}
pub struct EditFrame {
    editor: Editor,
    before: Snapshot,
    restored: bool,
}

pub fn prepare(ui: &egui::Ui, id: Id, text: &mut String) -> EditFrame {
    let ctx = ui.ctx();
    let mut editor = ctx.data_mut(|data| {
        data.get_temp::<Editor>(id.with("undo10"))
            .unwrap_or_default()
    });
    if editor.expected != *text {
        editor.undo.clear();
    }
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let mut restored = false;
    if ui.is_enabled()
        && ctx.memory(|memory| memory.has_focus(id))
        && ctx.input_mut(|input| {
            let mut undo = false;
            input.events.retain(|event| {
                if let egui::Event::Key {
                    key: Key::Z,
                    pressed: true,
                    modifiers,
                    ..
                } = event
                    && (modifiers.ctrl || modifiers.mac_cmd)
                    && !modifiers.alt
                    && !modifiers.shift
                {
                    undo = true;
                    false
                } else {
                    true
                }
            });
            undo
        })
        && let Some(previous) = editor.undo.pop()
    {
        *text = previous.text;
        state.cursor.set_char_range(Some(previous.cursor));
        restored = true;
    }
    // All undo belongs to our bounded stack; no hidden unbounded second history.
    state.clear_undoer();
    let cursor = state
        .cursor
        .char_range()
        .unwrap_or_else(|| CCursorRange::one(CCursor::new(text.chars().count())));
    state.store(ctx, id);
    EditFrame {
        editor,
        before: Snapshot {
            text: text.clone(),
            cursor,
        },
        restored,
    }
}
pub fn finish(ctx: &egui::Context, id: Id, mut frame: EditFrame, text: &str) -> bool {
    if frame.before.text != text {
        frame.editor.undo.push(frame.before);
    }
    frame.editor.expected = text.to_owned();
    ctx.data_mut(|data| data.insert_temp(id.with("undo10"), frame.editor));
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        state.clear_undoer();
        state.store(ctx, id);
    }
    frame.restored
}

#[cfg(test)]
mod tests {
    use super::*;
    fn edit_frame(ctx: &egui::Context, id: Id, text: &mut String, events: Vec<egui::Event>) {
        let _ = ctx.run(
            egui::RawInput {
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let before = prepare(ui, id, text);
                    egui::TextEdit::multiline(text).id(id).show(ui);
                    finish(ctx, id, before, text);
                });
            },
        );
    }
    fn key(key: Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: Some(key),
            pressed: true,
            repeat: false,
            modifiers,
        }
    }
    fn select(ctx: &egui::Context, id: Id, from: usize, to: usize) {
        ctx.memory_mut(|memory| memory.request_focus(id));
        let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(from),
            CCursor::new(to),
        )));
        state.store(ctx, id);
    }
    #[test]
    fn real_textedit_deletion_undo_restores_unicode_selection_and_caret() {
        let ctx = egui::Context::default();
        let id = Id::new("unicode_editor");
        let mut text = "árvíz 🦀 tűrő".to_owned();
        edit_frame(&ctx, id, &mut text, vec![]);
        select(&ctx, id, 2, 7);
        edit_frame(
            &ctx,
            id,
            &mut text,
            vec![key(Key::Backspace, egui::Modifiers::NONE)],
        );
        assert_eq!(text, "ár tűrő");
        assert_eq!(
            egui::TextEdit::load_state(&ctx, id)
                .unwrap()
                .cursor
                .char_range()
                .unwrap()
                .primary
                .index,
            2
        );
        edit_frame(
            &ctx,
            id,
            &mut text,
            vec![key(Key::Z, egui::Modifiers::CTRL)],
        );
        assert_eq!(text, "árvíz 🦀 tűrő");
        let range = egui::TextEdit::load_state(&ctx, id)
            .unwrap()
            .cursor
            .char_range()
            .unwrap();
        assert_eq!((range.primary.index, range.secondary.index), (7, 2));
    }
    #[test]
    fn real_editor_caps_ten_steps_and_external_clear_resets_history() {
        let ctx = egui::Context::default();
        let id = Id::new("bounded_editor");
        let mut text = String::new();
        edit_frame(&ctx, id, &mut text, vec![]);
        select(&ctx, id, 0, 0);
        for _ in 0..14 {
            edit_frame(&ctx, id, &mut text, vec![egui::Event::Text("á".into())]);
        }
        assert_eq!(text.chars().count(), 14);
        for _ in 0..11 {
            edit_frame(
                &ctx,
                id,
                &mut text,
                vec![key(Key::Z, egui::Modifiers::CTRL)],
            );
        }
        assert_eq!(text, "á".repeat(4));
        text.clear();
        edit_frame(
            &ctx,
            id,
            &mut text,
            vec![key(Key::Z, egui::Modifiers::CTRL)],
        );
        assert!(text.is_empty());
    }

    #[test]
    fn openrouter_multiline_selection_deletion_ignores_direction_and_ctrl_modifier() {
        for reverse in [false, true] {
            for modifiers in [egui::Modifiers::NONE, egui::Modifiers::CTRL] {
                let ctx = egui::Context::default();
                let id = Id::new("openrouter_multiline");
                let mut text = "első sor\nmásodik sor".to_owned();
                edit_frame(&ctx, id, &mut text, vec![]);
                let (a, b) = if reverse { (13, 5) } else { (5, 13) };
                select(&ctx, id, a, b);
                edit_frame(&ctx, id, &mut text, vec![key(Key::Backspace, modifiers)]);
                assert_eq!(text, "első dik sor");
                let cursor = egui::TextEdit::load_state(&ctx, id)
                    .unwrap()
                    .cursor
                    .char_range()
                    .unwrap();
                assert_eq!(cursor.primary.index, 5);
                assert!(cursor.is_empty());
                edit_frame(
                    &ctx,
                    id,
                    &mut text,
                    vec![key(Key::Z, egui::Modifiers::CTRL)],
                );
                assert_eq!(text, "első sor\nmásodik sor");
            }
        }
    }
    #[test]
    fn exactly_ten_steps_oldest_evicted_and_editors_independent() {
        let mut first = Undo::default();
        let mut second = Undo::default();
        second.push(99);
        for i in 0..14 {
            first.push(i);
        }
        for i in (4..14).rev() {
            assert_eq!(first.pop(), Some(i));
        }
        assert_eq!(first.pop(), None);
        assert_eq!(second.pop(), Some(99));
    }
    #[test]
    fn unicode_text_cursor_and_selection_survive_undo() {
        let original = Snapshot {
            text: "árvíz 🦀 tűrő".into(),
            cursor: CCursorRange::two(CCursor::new(3), CCursor::new(7)),
        };
        let mut undo = Undo::default();
        undo.push(original.clone());
        let restored = undo.pop().unwrap();
        assert_eq!(restored.text, original.text);
        assert_eq!(restored.cursor, original.cursor);
    }
}
