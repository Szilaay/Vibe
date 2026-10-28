//! Read-only limits from the native Codex account connected inside VIBE.
//! Uses the documented app-server protocol; no private usage endpoint or token copying.
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Result, anyhow, ensure};
use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct AccountUsageState {
    pub snapshot: Option<AccountUsage>,
    pub refreshing: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AccountUsage {
    pub primary: Option<UsageWindow>,
    pub secondary: Option<UsageWindow>,
    pub plan_type: Option<String>,
    pub fetched_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UsageWindow {
    pub remaining_percent: f64,
    pub window_seconds: Option<u64>,
    pub resets_at: Option<i64>,
}

pub async fn fetch() -> Result<AccountUsage> {
    crate::codex_account::rate_limits().await
}

pub(crate) fn from_app_server(response: &Value, plan_type: Option<String>) -> Result<AccountUsage> {
    parse(response, plan_type, now())
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs().min(i64::MAX as u64) as i64)
        .unwrap_or(0)
}

fn parse(response: &Value, account_plan: Option<String>, fetched_at: i64) -> Result<AccountUsage> {
    // Multi-bucket responses can contain unrelated product limits. Select Codex
    // explicitly; use the documented legacy bucket only when it identifies Codex
    // or omits the identifier (older CLI releases).
    let limits = response
        .pointer("/rateLimitsByLimitId/codex")
        .filter(|value| value.is_object())
        .or_else(|| {
            response.get("rateLimits").filter(|value| {
                value.is_object() && matches!(value["limitId"].as_str(), None | Some("codex"))
            })
        })
        .ok_or_else(|| anyhow!("Ehhez a fiókhoz nem érkezett Codex használati keret."))?;
    let primary = window(limits.get("primary"));
    let secondary = window(limits.get("secondary"));
    ensure!(
        primary.is_some() || secondary.is_some(),
        "Ehhez a fiókhoz nem érkezett mérhető Codex használati keret."
    );
    let plan_type = limits["planType"]
        .as_str()
        .map(str::to_owned)
        .or(account_plan)
        .filter(|plan| !plan.is_empty() && plan.len() <= 80 && !plan.chars().any(char::is_control));
    Ok(AccountUsage {
        primary,
        secondary,
        plan_type,
        fetched_at,
    })
}

fn window(value: Option<&Value>) -> Option<UsageWindow> {
    let value = value?;
    let used = value["usedPercent"]
        .as_f64()
        .filter(|used| used.is_finite())?;
    Some(UsageWindow {
        remaining_percent: (100.0 - used).clamp(0.0, 100.0),
        window_seconds: value["windowDurationMins"]
            .as_u64()
            .filter(|minutes| *minutes > 0)
            .and_then(|minutes| minutes.checked_mul(60)),
        resets_at: value["resetsAt"].as_i64().filter(|reset| *reset > 0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn native_windows_report_remaining_and_absolute_reset() {
        let usage = parse(
            &json!({"rateLimits": {
                "primary": { "usedPercent":35.5,"windowDurationMins":300,"resetsAt":2000 },
                "secondary": { "usedPercent":12,"windowDurationMins":10080,"resetsAt":4600 }
            }}),
            Some("plus".into()),
            1000,
        )
        .unwrap();
        let primary = usage.primary.unwrap();
        assert_eq!(primary.remaining_percent, 64.5);
        assert_eq!(primary.window_seconds, Some(18000));
        assert_eq!(primary.resets_at, Some(2000));
        assert_eq!(usage.secondary.unwrap().resets_at, Some(4600));
        assert_eq!(usage.plan_type.as_deref(), Some("plus"));
    }

    #[test]
    fn codex_bucket_wins_and_other_product_is_never_mislabeled() {
        let usage = parse(
            &json!({
                "rateLimits":{"primary":{"usedPercent":99}},
                "rateLimitsByLimitId":{
                    "other":{"primary":{"usedPercent":99}},
                    "codex":{"primary":{"usedPercent":25},"planType":"pro"}
                }
            }),
            None,
            1000,
        )
        .unwrap();
        assert_eq!(usage.primary.unwrap().remaining_percent, 75.0);
        assert_eq!(usage.plan_type.as_deref(), Some("pro"));
        assert!(
            parse(
                &json!({"rateLimits":{"limitId":"other","primary":{"usedPercent":99}}}),
                None,
                1000
            )
            .is_err()
        );
    }

    #[test]
    fn absent_null_and_missing_windows_are_unavailable_not_zero() {
        for payload in [
            json!({}),
            json!({"rateLimits":null}),
            json!({"rateLimits":{}}),
            json!({"rateLimits":{"primary":{"usedPercent":null}}}),
        ] {
            assert!(parse(&payload, None, 1000).is_err());
        }
        let partial = parse(
            &json!({"rateLimits":{"secondary":{"usedPercent":0}}}),
            None,
            1000,
        )
        .unwrap();
        assert!(partial.primary.is_none());
        assert_eq!(partial.secondary.unwrap().remaining_percent, 100.0);
    }

    #[test]
    fn out_of_range_usage_clamps_and_bad_window_never_overflows() {
        let usage = parse(
            &json!({"rateLimits":{
                "primary":{"usedPercent":120,"windowDurationMins":u64::MAX,"resetsAt":-1},
                "secondary":{"usedPercent":-1,"windowDurationMins":0}
            }}),
            None,
            1000,
        )
        .unwrap();
        assert_eq!(usage.primary.as_ref().unwrap().remaining_percent, 0.0);
        assert_eq!(usage.primary.as_ref().unwrap().window_seconds, None);
        assert_eq!(usage.primary.unwrap().resets_at, None);
        assert_eq!(usage.secondary.as_ref().unwrap().remaining_percent, 100.0);
        assert_eq!(usage.secondary.unwrap().window_seconds, None);
    }
}
