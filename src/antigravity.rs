//! The official Antigravity CLI owns Google Sign-In and its system-keyring session.
//! VIBE installs the verified official binary when missing, then launches `agy`
//! unchanged. Google tokens stay with the CLI and its native system keyring.
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use portable_pty::CommandBuilder;

pub use crate::antigravity_installer::{
    InstallProgress, ensure_installed, ensure_installed_with_progress,
};

/// Resolve PATH first, then the official installer's default directory.
/// Windows: `%LOCALAPPDATA%/agy/bin/agy.exe`; Unix: `$HOME/.local/bin/agy`.
/// Sources: https://antigravity.google/cli/install.ps1 and
/// https://antigravity.google/cli/install.sh. Custom installs belong on PATH.
pub fn resolve_executable() -> Result<PathBuf> {
    let path_dirs = std::env::var_os("PATH")
        .map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
        .unwrap_or_default();
    let default_dir = default_install_directory().ok();
    if let Some(executable) = resolve_in_dirs(&path_dirs, default_dir.as_deref()) {
        return Ok(executable);
    }
    bail!(
        "Antigravity CLI (agy) nem található. Telepítsd a hivatalos útmutató alapján: \
         https://antigravity.google/docs/cli/overview . Egyéni telepítésnél add az agy \
         könyvtárát a PATH-hoz, majd indítsd újra a VIBE-ot."
    )
}

pub(crate) fn default_install_directory() -> Result<PathBuf> {
    #[cfg(windows)]
    let directory = std::env::var_os("LOCALAPPDATA")
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join("agy").join("bin"));
    #[cfg(not(windows))]
    let directory = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(|value| PathBuf::from(value).join(".local").join("bin"));
    directory.filter(|path| path.is_absolute()).ok_or_else(|| {
        anyhow::anyhow!("Az Antigravity CLI telepítési könyvtára nem határozható meg.")
    })
}

fn resolve_in_dirs(path_dirs: &[PathBuf], default_dir: Option<&Path>) -> Option<PathBuf> {
    let name = if cfg!(windows) { "agy.exe" } else { "agy" };
    path_dirs
        .iter()
        .map(PathBuf::as_path)
        .chain(default_dir)
        // Never resolve an empty/relative PATH entry against a project directory.
        .filter(|directory| directory.is_absolute())
        .map(|directory| directory.join(name))
        .find(|candidate| is_executable(candidate))
}

pub(crate) fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

pub(crate) fn interactive_command(executable: &Path, cwd: &Path) -> CommandBuilder {
    let mut command = CommandBuilder::new(executable);
    crate::terminal_persistence::clear_hook_environment(&mut command);
    command.cwd(cwd);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    remove_provider_credentials(&mut command);
    // No flags, alternate auth, AGY_HOME override, or isolated user profile:
    // the original CLI uses its existing config and system keyring, opens the
    // browser for Google Sign-In when needed, and provides its own /logout.
    command
}

fn remove_provider_credentials(command: &mut CommandBuilder) {
    let names = command
        .iter_full_env_as_str()
        .map(|(name, _)| name.to_owned())
        .filter(|name| is_provider_credential(name))
        .collect::<Vec<_>>();
    for name in names {
        command.env_remove(name);
    }
}

/// Apply the same credential boundary to interactive and read-only CLI launches.
pub(crate) fn is_provider_credential(name: &str) -> bool {
    let name = name.to_ascii_uppercase();
    name.starts_with("OPENAI_")
        || name.starts_with("OPENROUTER_")
        || name.starts_with("GEMINI_")
        || (name.starts_with("GOOGLE_")
            && (name.ends_with("_API_KEY")
                || name.ends_with("_ACCESS_TOKEN")
                || name.ends_with("_AUTH_TOKEN")
                || name.ends_with("_CREDENTIALS")))
        || matches!(
            name.as_str(),
            "ACCESS_TOKEN"
                | "CODEX_API_KEY"
                | "GOOGLE_API_KEY"
                | "GOOGLE_APPLICATION_CREDENTIALS"
                | "GOOGLE_ACCESS_TOKEN"
                | "GOOGLE_OAUTH_ACCESS_TOKEN"
                | "GOOGLE_AUTH_TOKEN"
                | "GOOGLE_CLOUD_API_KEY"
                | "CLOUDSDK_AUTH_ACCESS_TOKEN"
                | "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    fn fake_executable(directory: &Path) -> PathBuf {
        std::fs::create_dir_all(directory).unwrap();
        let executable = directory.join(if cfg!(windows) { "agy.exe" } else { "agy" });
        std::fs::write(&executable, b"test fixture; never executed").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        executable
    }

    #[test]
    fn resolver_prefers_path_and_falls_back_to_official_default() {
        let root = tempfile::tempdir().unwrap();
        let path_dir = root.path().join("custom install");
        let default_dir = root.path().join("agy").join("bin");
        let default_executable = fake_executable(&default_dir);
        assert_eq!(
            resolve_in_dirs(std::slice::from_ref(&path_dir), Some(&default_dir)),
            Some(default_executable)
        );
        let path_executable = fake_executable(&path_dir);
        assert_eq!(
            resolve_in_dirs(&[path_dir], Some(&default_dir)),
            Some(path_executable)
        );
    }

    #[test]
    fn resolver_rejects_missing_files_directories_and_relative_path_entries() {
        let root = tempfile::tempdir().unwrap();
        let fake_directory = root
            .path()
            .join(if cfg!(windows) { "agy.exe" } else { "agy" });
        std::fs::create_dir(&fake_directory).unwrap();
        assert!(resolve_in_dirs(&[root.path().into()], None).is_none());
        assert!(resolve_in_dirs(&[PathBuf::new(), PathBuf::from(".")], None).is_none());
        assert!(resolve_in_dirs(&[], Some(&root.path().join("missing"))).is_none());
    }

    #[test]
    fn command_runs_original_cli_without_codex_flags_or_inline_credentials() {
        let command = interactive_command(Path::new("agy"), Path::new("project with spaces"));
        assert_eq!(command.get_argv(), &[OsStr::new("agy")]);
        assert_eq!(
            command.get_cwd().map(|cwd| cwd.as_os_str()),
            Some(OsStr::new("project with spaces"))
        );
        assert_eq!(command.get_env("TERM"), Some(OsStr::new("xterm-256color")));
        assert_eq!(command.get_env("COLORTERM"), Some(OsStr::new("truecolor")));
        assert!(command.get_env("ACCESS_TOKEN").is_none());
        assert!(command.get_env("OPENAI_API_KEY").is_none());
        assert!(command.get_env("GOOGLE_API_KEY").is_none());
    }

    #[test]
    fn inherited_provider_secrets_removed_but_native_profile_preserved() {
        let mut command = CommandBuilder::new("agy");
        command.env_clear();
        for name in [
            "ACCESS_TOKEN",
            "OPENAI_API_KEY",
            "OPENAI_ACCESS_TOKEN",
            "OPENROUTER_API_KEY",
            "GEMINI_API_KEY",
            "GEMINI_ACCESS_TOKEN",
            "CODEX_API_KEY",
            "GOOGLE_API_KEY",
            "GOOGLE_APPLICATION_CREDENTIALS",
            "GOOGLE_OAUTH_ACCESS_TOKEN",
            "GOOGLE_AUTH_TOKEN",
            "GOOGLE_CLOUD_API_KEY",
            "CLOUDSDK_AUTH_ACCESS_TOKEN",
            "CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE",
        ] {
            command.env(name, "synthetic-secret");
        }
        for name in [
            "AGY_HOME",
            "HOME",
            "USERPROFILE",
            "LOCALAPPDATA",
            "GOOGLE_CLOUD_PROJECT",
            "PATH",
        ] {
            command.env(name, "synthetic-native-profile");
        }
        remove_provider_credentials(&mut command);
        assert_eq!(command.iter_full_env_as_str().count(), 6);
        for name in [
            "AGY_HOME",
            "HOME",
            "USERPROFILE",
            "LOCALAPPDATA",
            "GOOGLE_CLOUD_PROJECT",
            "PATH",
        ] {
            assert_eq!(
                command.get_env(name),
                Some(OsStr::new("synthetic-native-profile"))
            );
        }
    }
}
