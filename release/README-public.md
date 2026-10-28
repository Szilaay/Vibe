# VIBE

Windows desktop workspace for independent Codex and Antigravity CLI conversations, plus a workspace-aware OpenRouter coding agent.

- OpenRouter terminal-style interface, workspace file browsing, file attachments and reviewed edits.
- Workspace-specific agent rules with optional inheritance from global VIBE rules.
- Input selection and bounded undo. In native CLI panels, regular dragging selects input only; Shift + drag and Ctrl+C copy previous output without modifying it.

## Download

Open [the latest release](https://github.com/Szilaay/Vibe/releases/latest) and download **VIBE-Setup-x.y.z.exe**.

Run the installer. It installs VIBE, the offline Hungarian dictation model, and required runtime libraries for the current Windows user. Windows 10/11 x64 is required.

## Updates

Starting with version 0.2.0, VIBE checks for signed release metadata shortly after startup and then at most once every six hours. You can also use **Account & settings → Frissítések keresése…**.

New versions are offered inside the app. Download and installation require your confirmation. VIBE verifies an Ed25519 signature and the installer's SHA-256 checksum before offering installation. Close the old app if the installer asks you to.

Account settings, conversations, and terminal snapshots are stored separately from the program and are preserved when updating or uninstalling. The older portable versions require a one-time manual installation of this installer-enabled version.

## Notes

- This repository contains VIBE source code under the MIT license. Installers and signed update metadata are published on the Releases page.
- The installer is currently **not Windows Authenticode code-signed**; the update signature is separate from Windows publisher signing.
- Codex and Antigravity use their own account sign-in and model access requirements.
- Offline dictation processes microphone audio locally.

The offline engine/model licenses and Microsoft runtime notices are included with the installed files.
