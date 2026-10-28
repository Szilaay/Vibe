Upstream: egui-winit 0.33.3, https://github.com/emilk/egui
License: MIT OR Apache-2.0 (as declared in Cargo.toml).

The keyboard adapter normally consumes Ctrl+V even when the clipboard has no
text. This loses image-only screenshots before the terminal sees the shortcut.
Our patch preserves the original key event when no nonempty text is available.
Text paste remains an Event::Paste. The terminal encodes Ctrl+V as 0x16; the
original Codex CLI reads and attaches the image using its own clipboard support.
Only the focused terminal accepts this event. No clipboard polling, image
encoding, extra copies, or model submission runs in VIBE's UI thread.

Keep this patch when upgrading egui-winit, or remove it once upstream exposes
image-only paste events. Tests live in src/terminal_widget.rs.
