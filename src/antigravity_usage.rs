//! Read-only `/usage` output from the original Antigravity CLI.
//! The print-mode quota groups/buckets match v1.4.2's public JSON structs.
//! No account files are read, no Google tokens are copied, no agent turn is sent.
use std::{
    path::Path,
    process::Stdio,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    process::Command,
};

const MAX_OUTPUT: usize = 512 * 1024;
const AUTH_REQUIRED: &str = "Az Antigravity használati adataihoz előbb jelentkezz be Google-fiókkal az Antigravity CLI-ben.";

#[derive(Clone, Debug, Default)]
pub struct AntigravityUsageState {
    pub snapshot: Option<AntigravityUsage>,
    pub refreshing: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
pub struct AntigravityUsage {
    pub models: Vec<ModelQuota>,
    pub fetched_at: i64,
    pub account_label: Option<String>,
    pub plan_label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ModelQuota {
    pub name: String,
    pub remaining_percent: Option<f64>,
    pub remaining_amount: Option<u64>,
    pub reset_at: Option<i64>,
    pub reset_label: Option<String>,
}

pub async fn fetch_usage() -> Result<AntigravityUsage> {
    let executable = crate::antigravity::resolve_executable()?;
    let directory = tempfile::Builder::new()
        .prefix("vibe-agy-usage-")
        .tempdir()?;
    // Older releases could interpret slash text as a model prompt. Never run
    // /usage unless this CLI version explicitly supports read-only print mode.
    let mut version = command(&executable, directory.path());
    version.arg("--version");
    let version = capture(version, Duration::from_secs(10)).await?;
    ensure!(
        supports_usage(&String::from_utf8_lossy(&version)),
        "Antigravity usage: legalább CLI 1.1.11 szükséges; frissítsd az agy CLI-t."
    );
    let mut usage = command(&executable, directory.path());
    usage.args([
        "-p",
        "/usage",
        "--output-format",
        "json",
        "--print-timeout",
        "20s",
    ]);
    let output = capture(usage, Duration::from_secs(30)).await?;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    parse(&output, now)
}

fn command(executable: &Path, directory: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .current_dir(directory)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("NO_COLOR", "1")
        .env("TERM", "dumb")
        .env("AGY_CLI_OSC7501", "off");
    for (name, _) in std::env::vars_os() {
        if crate::antigravity::is_provider_credential(&name.to_string_lossy()) {
            command.env_remove(name);
        }
    }
    #[cfg(windows)]
    command.creation_flags(0x0800_0000);
    command
}

fn supports_usage(version: &str) -> bool {
    version
        .split_whitespace()
        .find_map(|word| {
            let mut numbers = word.trim_start_matches('v').split('.');
            let major: u32 = numbers.next()?.parse().ok()?;
            let minor: u32 = numbers.next()?.parse().ok()?;
            let patch: u32 = numbers.next()?.split(['-', '+']).next()?.parse().ok()?;
            Some((major, minor, patch) >= (1, 1, 11))
        })
        .unwrap_or(false)
}

async fn capture(mut command: Command, timeout: Duration) -> Result<Vec<u8>> {
    let mut child = command
        .spawn()
        .context("Az Antigravity usage-lekérdezése nem indítható.")?;
    let stdout = child
        .stdout
        .take()
        .context("Antigravity stdout hiányzik.")?;
    let stderr = child
        .stderr
        .take()
        .context("Antigravity stderr hiányzik.")?;
    let result = tokio::time::timeout(timeout, async {
        let (stdout, _stderr, status) = tokio::try_join!(
            read_output(stdout, MAX_OUTPUT), read_output(stderr, MAX_OUTPUT),
            async { child.wait().await.context("Az Antigravity folyamat megszakadt.") }
        )?;
        // Never display raw diagnostics: authentication prompts contain URLs
        // and the provider can also include account details in error output.
        ensure!(status.success(), "Az Antigravity nem tudta lekérni a használati kereteket. Ellenőrizd a CLI-ben a Google-belépést és a /usage parancsot.");
        Ok(stdout)
    }).await;
    let result = result.unwrap_or_else(|_| Err(anyhow::anyhow!("Antigravity usage: időtúllépés.")));
    if result.is_err() {
        let _ = child.kill().await;
        let _ = child.wait().await;
    }
    result
}

async fn read_output(mut reader: impl AsyncRead + Unpin, limit: usize) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0; 4096];
    loop {
        let length = reader
            .read(&mut chunk)
            .await
            .context("Antigravity válasz olvasási hiba.")?;
        if length == 0 {
            return Ok(output);
        }
        ensure!(
            output.len().saturating_add(length) <= limit,
            "Az Antigravity usage válasza túl nagy."
        );
        output.extend_from_slice(&chunk[..length]);
        let tail = String::from_utf8_lossy(&output[output.len().saturating_sub(8192)..])
            .to_ascii_lowercase();
        ensure!(
            ![
                "authentication required",
                "waiting for authentication",
                "accounts.google.com/o/oauth2",
                "paste the authorization code"
            ]
            .iter()
            .any(|marker| tail.contains(marker)),
            AUTH_REQUIRED
        );
    }
}

fn text(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|text| !text.is_empty() && text.len() <= 320 && !text.chars().any(char::is_control))
        .map(str::to_owned)
}

fn parse(bytes: &[u8], fetched_at: i64) -> Result<AntigravityUsage> {
    let value: Value =
        serde_json::from_slice(bytes).context("Ismeretlen Antigravity usage JSON-formátum.")?;
    parse_value(&value, fetched_at, 0)
}

fn parse_value(value: &Value, fetched_at: i64, depth: usize) -> Result<AntigravityUsage> {
    ensure!(depth <= 4, "Az Antigravity usage-válasz túl mély.");
    ensure!(
        value
            .get("num_turns")
            .is_none_or(|turns| turns.as_u64() == Some(0)),
        "Az Antigravity nem csak olvasó usage-választ adott."
    );
    if let Some(status) = value.get("status") {
        ensure!(
            status
                .as_str()
                .is_some_and(|status| status.eq_ignore_ascii_case("success")),
            "Az Antigravity usage-lekérdezése sikertelen. Ellenőrizd a CLI-ben a Google-belépést és a /usage parancsot."
        );
    }
    // agy 1.4.2 returns a print result envelope. `response` is a human-readable
    // summary, while the authoritative quotas live in command.data.groups.
    if let Some(command) = value.get("command").filter(|command| !command.is_null()) {
        ensure!(
            matches!(command["name"].as_str(), Some("usage" | "quota")),
            "Az Antigravity nem usage-parancsra adott választ."
        );
        let data = command
            .get("data")
            .filter(|data| data.is_object())
            .context("Hiányzó Antigravity usage-parancsadatok.")?;
        return parse_value(data, fetched_at, depth + 1);
    }
    if let Some(groups) = value["groups"].as_array() {
        ensure!(groups.len() <= 64, "Túl sok Antigravity kvótacsoport.");
        let mut models = Vec::new();
        for group in groups {
            let group_name = text(&group["name"]).unwrap_or_else(|| "Model quota".into());
            let Some(buckets) = group["buckets"].as_array() else {
                continue;
            };
            ensure!(buckets.len() <= 128, "Túl sok Antigravity kvótakeret.");
            for bucket in buckets {
                let Some(bucket_name) = text(&bucket["name"]) else {
                    continue;
                };
                let disabled = bucket["disabled"].as_bool().unwrap_or(false);
                let remaining_percent = (!disabled)
                    .then(|| bucket["remaining_fraction"].as_f64())
                    .flatten()
                    .filter(|value| value.is_finite() && (0.0..=1.0).contains(value))
                    .map(|value| value * 100.0);
                let name = if bucket_name == group_name {
                    group_name.clone()
                } else {
                    format!("{group_name} · {bucket_name}")
                };
                let window = text(&bucket["window"]);
                let name = if let Some(window) = window {
                    format!("{name} ({window})")
                } else {
                    name
                };
                let reset = text(&bucket["reset_time"]);
                let reset_at = reset
                    .as_deref()
                    .and_then(|value| {
                        time::OffsetDateTime::parse(
                            value,
                            &time::format_description::well_known::Rfc3339,
                        )
                        .ok()
                    })
                    .map(|value| value.unix_timestamp());
                let reset_label = if disabled {
                    Some("Disabled".into())
                } else {
                    reset
                        .filter(|_| reset_at.is_none())
                        .map(|value| format!("Reset: {value}"))
                };
                models.push(ModelQuota {
                    name,
                    remaining_percent,
                    remaining_amount: (!disabled)
                        .then(|| bucket["remaining_amount"].as_u64())
                        .flatten(),
                    reset_at: if disabled { None } else { reset_at },
                    reset_label,
                });
                ensure!(models.len() <= 256, "Túl sok Antigravity kvótakeret.");
            }
        }
        ensure!(
            !models.is_empty(),
            "Az Antigravity nem jelentett modellkereteket."
        );
        return Ok(AntigravityUsage {
            models,
            fetched_at,
            account_label: None,
            plan_label: None,
        });
    }
    // Read-only print releases may wrap their structured command payload.
    for key in ["data", "result", "quota", "structured_output"] {
        if let Some(nested) = value.get(key).filter(|value| value.is_object())
            && let Ok(usage) = parse_value(nested, fetched_at, depth + 1)
        {
            return Ok(usage);
        }
    }
    if let Some(response) = value["response"].as_str()
        && let Ok(nested) = serde_json::from_str::<Value>(response)
    {
        return parse_value(&nested, fetched_at, depth + 1);
    }
    bail!("Ismeretlen Antigravity usage-formátum. Ellenőrizd a CLI-ben a /usage parancsot.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cli_142_command_envelope_reads_quotas_instead_of_plain_response() {
        let value = json!({
            "conversation_id": "synthetic",
            "status": "SUCCESS",
            "response": "Model quota status",
            "duration_seconds": 1,
            "num_turns": 0,
            "usage": {"input_tokens": 0, "output_tokens": 0},
            "command": {"name": "usage", "data": {
                "description": "Quota status",
                "groups": [
                    {"name": "Gemini Models", "buckets": [
                        {"id": "gemini-weekly", "name": "Weekly Limit Remaining", "window": "weekly",
                         "remaining_fraction": 0.8961808085441589, "reset_time": "2026-10-26T19:55:44Z"}
                    ]},
                    {"name": "Claude and GPT models", "buckets": [
                        {"name": "Weekly Limit Remaining", "window": "weekly", "remaining_fraction": 1.0,
                         "reset_time": "2026-10-27T03:34:01Z"}
                    ]}
                ]
            }}
        });
        let usage = parse(value.to_string().as_bytes(), 123).unwrap();
        assert_eq!(usage.models.len(), 2);
        assert!((usage.models[0].remaining_percent.unwrap() - 89.618_081).abs() < 0.00001);
        assert_eq!(usage.models[1].remaining_percent, Some(100.0));
        assert!(usage.models.iter().all(|model| model.reset_at.is_some()));
        assert!(usage.models[0].name.contains("Gemini Models"));
        assert_eq!(usage.fetched_at, 123);
    }

    #[test]
    fn command_envelope_rejects_failed_unrelated_or_non_read_only_results() {
        let data = json!({"groups": [{"name": "Test", "buckets": [{"name": "Weekly", "remaining_fraction": 1.0}]}]});
        for (status, turns, name) in [
            (json!("ERROR"), json!(0), "usage"),
            (json!("CANCELLED"), json!(0), "usage"),
            (json!("SUCCESS"), json!(1), "usage"),
            (json!("SUCCESS"), json!(-1), "usage"),
            (json!("SUCCESS"), json!("0"), "usage"),
            (json!("SUCCESS"), json!(0), "model"),
        ] {
            let value = json!({"status": status, "num_turns": turns, "command": {"name": name, "data": data}});
            assert!(parse(value.to_string().as_bytes(), 0).is_err());
        }
        let alias = json!({"status": "SUCCESS", "num_turns": 0, "command": {"name": "quota", "data": data}});
        assert!(parse(alias.to_string().as_bytes(), 0).is_ok());
    }

    #[tokio::test]
    #[ignore = "Requires installed Antigravity CLI and Google login; read-only live quota request"]
    async fn installed_cli_reads_usage_without_agent_turn() {
        let usage = fetch_usage()
            .await
            .expect("The installed CLI must return readable quota data");
        assert!(!usage.models.is_empty());
        println!(
            "Read {} quota buckets from the installed CLI",
            usage.models.len()
        );
    }

    #[test]
    fn actual_print_struct_shape_preserves_each_bucket_and_reset() {
        let value = json!({"groups":[{"name":"Gemini Pro","buckets":[
            {"name":"Requests","window":"5h","remaining_fraction":0.65,"reset_time":"2026-10-20T12:00:00Z"},
            {"name":"Weekly","remaining_fraction":0.0},
            {"name":"Unknown","remaining_amount":42},
            {"name":"Disabled","disabled":true,"remaining_fraction":1.0}
        ]}]});
        let usage = parse(value.to_string().as_bytes(), 123).unwrap();
        assert_eq!(usage.models.len(), 4);
        assert_eq!(usage.models[0].remaining_percent, Some(65.0));
        assert!(usage.models[0].reset_at.is_some());
        assert_eq!(usage.models[1].remaining_percent, Some(0.0));
        assert!(usage.models[2].remaining_percent.is_none());
        assert_eq!(usage.models[2].remaining_amount, Some(42));
        assert!(usage.models[3].remaining_percent.is_none());
        assert_eq!(usage.models[3].reset_label.as_deref(), Some("Disabled"));
        assert!(usage.account_label.is_none());
        assert_eq!(usage.fetched_at, 123);
    }

    #[test]
    fn invalid_empty_or_model_responses_are_not_quota() {
        for value in [
            json!({}),
            json!({"groups":[]}),
            json!({"usage":{"total_tokens":0}}),
            json!({"num_turns":1,"response":"{\"groups\":[]}"}),
        ] {
            assert!(parse(value.to_string().as_bytes(), 0).is_err());
        }
        let usage = parse(br#"{"data":{"groups":[{"name":"Test","buckets":[{"name":"Daily","remaining_fraction":null}]}]}}"#, 0).unwrap();
        assert!(usage.models[0].remaining_percent.is_none());
    }

    #[test]
    fn old_or_unrecognized_versions_never_run_print_prompt() {
        for version in ["1.1.10", "0.99.0", "unknown", ""] {
            assert!(!supports_usage(version));
        }
        for version in ["1.1.11", "1.4.2\n", "Antigravity CLI v2.0.0"] {
            assert!(supports_usage(version));
        }
    }

    #[tokio::test]
    async fn auth_prompt_aborts_without_waiting_for_login_or_leaking_link() {
        let (mut writer, reader) = tokio::io::duplex(4096);
        use tokio::io::AsyncWriteExt;
        writer.write_all(b"Authentication required. https://accounts.google.com/o/oauth2/auth?state=synthetic").await.unwrap();
        let error =
            tokio::time::timeout(Duration::from_millis(500), read_output(reader, MAX_OUTPUT))
                .await
                .unwrap()
                .unwrap_err();
        assert_eq!(error.to_string(), AUTH_REQUIRED);
        assert!(!error.to_string().contains("synthetic"));
    }

    #[tokio::test]
    async fn output_limit_is_enforced_before_parsing() {
        assert!(read_output(&b"0123456789"[..], 5).await.is_err());
    }
}
