//! Official managed Codex sign-in, isolated in VIBE's own persistent profile.
//! No other application's account store or credentials are read or imported.
//! Protocol: https://learn.chatgpt.com/docs/app-server
use std::{path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use directories::ProjectDirs;
use serde_json::{Value, json};
use tokio::{
    process::{Child, ChildStdin, ChildStdout, Command},
    sync::{Mutex, mpsc::UnboundedSender},
    task::JoinHandle,
};

use crate::{account_usage::AccountUsage, codex_runtime::Rpc};

static ACCOUNT_OPERATION: Mutex<()> = Mutex::const_new(());
const LOGIN_TIMEOUT: Duration = Duration::from_secs(300);

/// Apply this option to every native TUI using this profile, too. It prevents
/// native Codex from consulting a system keyring shared with other installations.
pub const CREDENTIAL_STORE_SETTING: &str = "cli_auth_credentials_store=\"file\"";

/// Credentials inherited from a launcher must never select a different account.
pub const CREDENTIAL_ENV: &[&str] = &[
    "ACCESS_TOKEN",
    "OPENAI_API_KEY",
    "OPENAI_ACCESS_TOKEN",
    "CODEX_API_KEY",
    "CODEX_ACCESS_TOKEN",
    "OPENROUTER_API_KEY",
    "OPENAI_BASE_URL",
    "OPENAI_FEDERATION_RULE_ID",
    "OPENAI_IDENTITY_TOKEN_FILE",
    "CODEX_INTERNAL_ORIGINATOR_OVERRIDE",
    "CODEX_WIF_TOKEN_FILE",
    "CODEX_WIF_TOKEN_FD",
    "CODEX_WIF_FEDERATION_RULE_ID",
];

#[derive(Clone, Debug, PartialEq)]
pub struct AccountInfo {
    pub email: Option<String>,
    pub plan_type: Option<String>,
}

pub enum LoginEvent {
    Waiting { authorization_url: String },
    Completed(AccountInfo),
    Failed(String),
}

pub fn home() -> Result<PathBuf> {
    let directories = ProjectDirs::from("dev", "Vibe", "Vibe")
        .context("VIBE saját Codex adatkönyvtára nem érhető el")?;
    Ok(directories.data_local_dir().join("native-codex"))
}

/// Startup reads only VIBE's own saved profile. Do not start a CLI just to draw
/// the window: app-server initialization may run provider networking or plugins.
/// Saved tokens are not validated here; the native CLI validates them on use.
pub async fn saved_account() -> Result<Option<AccountInfo>> {
    let profile = home()?;
    tokio::task::spawn_blocking(move || saved_account_at(&profile))
        .await
        .context("Mentett Codex-fiók olvasása megszakadt")?
}

fn saved_account_at(profile: &std::path::Path) -> Result<Option<AccountInfo>> {
    use std::io::Read;
    let file = match std::fs::File::open(profile.join("auth.json")) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => bail!("Mentett Codex-fiók nem olvasható"),
    };
    const MAX_AUTH_BYTES: u64 = 1024 * 1024;
    let mut bytes = Vec::new();
    file.take(MAX_AUTH_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("Mentett Codex-fiók nem olvasható")?;
    ensure!(
        bytes.len() as u64 <= MAX_AUTH_BYTES,
        "Mentett Codex-fájl túl nagy"
    );
    // Do not expose JSON parsing diagnostics: the input contains credentials.
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("Mentett Codex-fájl érvénytelen"))?;
    if value["auth_mode"]
        .as_str()
        .is_some_and(|mode| mode != "chatgpt")
        || !["access_token", "refresh_token"].iter().all(|key| {
            value["tokens"][key]
                .as_str()
                .is_some_and(|token| !token.is_empty())
        })
    {
        return Ok(None);
    }
    // JWT claims here are display hints only, never proof of authentication.
    use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
    let claims = value["tokens"]["id_token"]
        .as_str()
        .and_then(|token| token.split('.').nth(1))
        .and_then(|payload| URL_SAFE_NO_PAD.decode(payload).ok())
        .and_then(|payload| serde_json::from_slice::<Value>(&payload).ok())
        .unwrap_or(Value::Null);
    Ok(Some(AccountInfo {
        email: display_text(claims["email"].as_str()),
        plan_type: display_text(
            claims["https://api.openai.com/auth"]["chatgpt_plan_type"].as_str(),
        ),
    }))
}

pub async fn account() -> Result<Option<AccountInfo>> {
    let _operation = ACCOUNT_OPERATION.lock().await;
    // An untouched installation has no native account. No CLI startup necessary.
    if !home()?.exists() {
        return Ok(None);
    }
    let mut server = AccountServer::start().await?;
    let result = server.account().await;
    server.stop().await;
    result
}

/// Call only after a user chooses to connect. The UI opens the returned official
/// authorization URL; the CLI owns OAuth, its callback and token persistence.
pub async fn login(sender: UnboundedSender<LoginEvent>) {
    let _operation = ACCOUNT_OPERATION.lock().await;
    let result = login_inner(&sender).await;
    let event = match result {
        Ok(account) => LoginEvent::Completed(account),
        Err(error) => LoginEvent::Failed(error.to_string()),
    };
    let _ = sender.send(event);
}

async fn login_inner(sender: &UnboundedSender<LoginEvent>) -> Result<AccountInfo> {
    let mut server = AccountServer::start().await?;
    let result = async {
        let response = server.rpc.call("account/login/start", json!({"type":"chatgpt"})).await?;
        let login_id = response["loginId"].as_str().context("Codex bejelentkezési azonosító hiányzik")?.to_owned();
        let authorization_url = response["authUrl"].as_str().context("Codex bejelentkezési URL hiányzik")?;
        validate_authorization_url(authorization_url)?;
        sender.send(LoginEvent::Waiting { authorization_url: authorization_url.to_owned() })
            .map_err(|_| anyhow::anyhow!("Codex bejelentkezés megszakítva."))?;
        // Only reached after the user explicitly selected Connect in VIBE.
        // Keep the URL event available as a manual fallback if browser launch fails.
        let _ = open::that_detached(authorization_url);
        let wait = tokio::time::timeout(LOGIN_TIMEOUT, async {
            loop {
                let incoming = server.rpc.next().await?;
                if server.rpc.reject_server_request(&incoming).await? { continue; }
                if incoming["method"] != "account/login/completed"
                    || incoming["params"]["loginId"].as_str() != Some(login_id.as_str()) {
                    continue;
                }
                ensure!(incoming["params"]["success"] == true, "Codex bejelentkezés sikertelen vagy megszakítva.");
                return server.account().await?.context("Codex bejelentkezés után nem érkezett fiókadat.");
            }
        });
        tokio::select! {
            result = wait => match result {
                Ok(result) => result,
                Err(_) => {
                    let _ = server.rpc.call("account/login/cancel", json!({"loginId":login_id})).await;
                    bail!("Codex bejelentkezés: időtúllépés.")
                }
            },
            _ = sender.closed() => {
                // Dropping the server kills its pending local callback listener.
                bail!("Codex bejelentkezés megszakítva.")
            }
        }
    }.await;
    server.stop().await;
    result
}

/// Call only after a user chooses to disconnect, after stopping native panels.
pub async fn logout() -> Result<()> {
    let _operation = ACCOUNT_OPERATION.lock().await;
    if !home()?.exists() {
        return Ok(());
    }
    let mut server = AccountServer::start().await?;
    let result = server
        .rpc
        .call("account/logout", json!({}))
        .await
        .map(|_| ());
    server.stop().await;
    result
}

pub async fn rate_limits() -> Result<AccountUsage> {
    let _operation = ACCOUNT_OPERATION.lock().await;
    ensure!(
        home()?.exists(),
        "A használati kerethez csatlakoztasd a Codex-fiókot a VIBE-ban."
    );
    let mut server = AccountServer::start().await?;
    let result = async {
        let account = server
            .account()
            .await?
            .context("A használati kerethez csatlakoztasd a Codex-fiókot a VIBE-ban.")?;
        let response = server
            .rpc
            .call("account/rateLimits/read", json!({}))
            .await?;
        crate::account_usage::from_app_server(&response, account.plan_type)
    }
    .await;
    server.stop().await;
    result
}

fn validate_authorization_url(value: &str) -> Result<()> {
    let url = url::Url::parse(value).context("Codex bejelentkezési URL érvénytelen")?;
    ensure!(
        url.scheme() == "https"
            && matches!(url.host_str(), Some("auth.openai.com" | "chatgpt.com"))
            && url.port_or_known_default() == Some(443)
            && url.username().is_empty()
            && url.password().is_none(),
        "Codex bejelentkezési URL nem hivatalos OpenAI-cím."
    );
    ensure!(
        matches!(url.path(), "/oauth/authorize" | "/api/accounts/authorize"),
        "Codex bejelentkezési URL útvonala nem támogatott."
    );
    Ok(())
}

fn parse_account(response: &Value) -> Result<Option<AccountInfo>> {
    let Some(account) = response.get("account").filter(|account| !account.is_null()) else {
        return Ok(None);
    };
    ensure!(
        account["type"] == "chatgpt",
        "A VIBE saját Codex-profilja nem ChatGPT-fiókot használ."
    );
    Ok(Some(AccountInfo {
        email: display_text(account["email"].as_str()),
        plan_type: display_text(account["planType"].as_str()),
    }))
}

fn display_text(value: Option<&str>) -> Option<String> {
    value
        .filter(|text| !text.is_empty() && text.len() <= 320 && !text.chars().any(char::is_control))
        .map(str::to_owned)
}

struct AccountServer {
    child: Child,
    rpc: Rpc<ChildStdout, ChildStdin>,
    stderr: JoinHandle<()>,
}

impl AccountServer {
    async fn start() -> Result<Self> {
        let profile = home()?;
        Self::start_at(&profile).await
    }

    async fn start_at(profile: &std::path::Path) -> Result<Self> {
        std::fs::create_dir_all(profile)
            .context("VIBE saját Codex adatkönyvtára nem hozható létre")?;
        let mut command = Command::new(crate::codex_runtime::resolve_executable()?);
        command
            .args([
                "--no-daemon",
                "app-server",
                "--listen",
                "stdio://",
                "-c",
                CREDENTIAL_STORE_SETTING,
                "-c",
                "model_provider=\"openai\"",
            ])
            .env("CODEX_HOME", profile)
            .current_dir(profile)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for variable in CREDENTIAL_ENV {
            command.env_remove(variable);
        }
        #[cfg(windows)]
        command.creation_flags(0x0800_0000);
        let mut child = command.spawn().context("Codex CLI nem indítható")?;
        let input = child.stdin.take().context("Codex CLI stdin hiányzik")?;
        let output = child.stdout.take().context("Codex CLI stdout hiányzik")?;
        let mut diagnostic = child.stderr.take().context("Codex CLI stderr hiányzik")?;
        let stderr = tokio::spawn(async move {
            // Native diagnostics may contain account details; never persist them.
            let _ = tokio::io::copy(&mut diagnostic, &mut tokio::io::sink()).await;
        });
        let mut server = Self {
            child,
            rpc: Rpc::new(output, input),
            stderr,
        };
        server.rpc.call("initialize", json!({"clientInfo":{"name":"vibe","title":"VIBE","version":env!("CARGO_PKG_VERSION")}})).await?;
        server.rpc.write(json!({"method":"initialized"})).await?;
        Ok(server)
    }

    async fn account(&mut self) -> Result<Option<AccountInfo>> {
        parse_account(
            &self
                .rpc
                .call("account/read", json!({"refreshToken":false}))
                .await?,
        )
    }

    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        self.stderr.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_urls_require_exact_official_https_host() {
        for value in [
            "https://auth.openai.com/oauth/authorize?state=abc",
            "https://chatgpt.com/api/accounts/authorize",
        ] {
            assert!(validate_authorization_url(value).is_ok());
        }
        for value in [
            "http://auth.openai.com/",
            "https://auth.openai.com.evil.test/",
            "https://evil.test/",
            "https://user@auth.openai.com/",
            "https://auth.openai.com:444/",
            "https://auth.openai.com/arbitrary",
        ] {
            assert!(validate_authorization_url(value).is_err());
        }
    }

    #[test]
    fn startup_account_read_is_local_bounded_and_tolerates_missing_display_claims() {
        let profile = tempfile::tempdir().unwrap();
        assert_eq!(saved_account_at(profile.path()).unwrap(), None);
        let path = profile.path().join("auth.json");
        std::fs::write(&path, br#"{"auth_mode":"chatgpt","tokens":{"access_token":"synthetic","refresh_token":"synthetic","id_token":"invalid"}}"#).unwrap();
        assert_eq!(
            saved_account_at(profile.path()).unwrap(),
            Some(AccountInfo {
                email: None,
                plan_type: None
            })
        );
        std::fs::write(&path, br#"{"auth_mode":"apikey","tokens":{"access_token":"synthetic","refresh_token":"synthetic"}}"#).unwrap();
        assert_eq!(saved_account_at(profile.path()).unwrap(), None);
        std::fs::write(&path, br#"{"tokens":{"access_token":"synthetic"}}"#).unwrap();
        assert_eq!(saved_account_at(profile.path()).unwrap(), None);
        std::fs::write(&path, b"secret-invalid-json").unwrap();
        assert!(
            !saved_account_at(profile.path())
                .unwrap_err()
                .to_string()
                .contains("secret")
        );
        std::fs::write(&path, vec![b'x'; 1024 * 1024 + 1]).unwrap();
        assert!(saved_account_at(profile.path()).is_err());
    }

    #[test]
    fn account_identity_comes_only_from_native_profile() {
        assert_eq!(parse_account(&json!({"account":null})).unwrap(), None);
        assert!(parse_account(&json!({"account":{"type":"apiKey"}})).is_err());
        let account = parse_account(
            &json!({"account":{"type":"chatgpt","email":"native@example.test","planType":"plus"}}),
        )
        .unwrap()
        .unwrap();
        assert_eq!(account.email.as_deref(), Some("native@example.test"));
        assert_eq!(account.plan_type.as_deref(), Some("plus"));
        assert!(home().unwrap().ends_with("native-codex"));
        assert!(CREDENTIAL_ENV.contains(&"CODEX_ACCESS_TOKEN"));
    }

    #[tokio::test]
    #[ignore = "Requires installed Codex CLI; new temporary profile, no login or inference"]
    async fn installed_cli_reads_only_empty_isolated_account() {
        let profile = tempfile::tempdir().unwrap();
        let mut server = AccountServer::start_at(profile.path()).await.unwrap();
        assert_eq!(server.account().await.unwrap(), None);
        server.stop().await;
    }
}
