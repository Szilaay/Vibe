//! ChatGPT plan usage through the official public Responses API.
//! https://developers.openai.com/siwc/token-sharing-open-source/models-and-inference

use std::time::Duration;

use anyhow::{Result, anyhow};
use futures_util::{Stream, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::{Role, Usage};
use crate::provider::{ChatRequest, ProviderEvent, SseEvent, SseParser};

const API: &str = "https://api.openai.com/v1";
const INACTIVITY: Duration = Duration::from_secs(90);
const MAX_INPUT: usize = 1024 * 1024;
const MAX_OUTPUT: usize = 1024 * 1024;
const MAX_STREAM: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| anyhow!("ChatGPT HTTP kliens létrehozása sikertelen."))
}

fn payload(request: &ChatRequest) -> Result<Value> {
    if request.model.is_empty()
        || request.model.len() > 160
        || request.model.chars().any(char::is_control)
    {
        return Err(anyhow!("Érvénytelen ChatGPT modellazonosító."));
    }
    let mut size = 0usize;
    let mut input = Vec::new();
    for message in &request.messages {
        if message.role == Role::System
            || (message.role == Role::Assistant && message.content.trim().is_empty())
        {
            continue;
        }
        size = size
            .checked_add(message.content.len())
            .ok_or_else(|| anyhow!("Beszélgetés túl nagy."))?;
        if size > MAX_INPUT || input.len() >= 4096 {
            return Err(anyhow!(
                "ChatGPT beszélgetés túllépte az 1 MiB vagy 4096 üzenet korlátot."
            ));
        }
        // EasyInputMessage accepts plain text content for both user and assistant.
        input.push(json!({"role": message.role, "content": message.content}));
    }
    if input.is_empty() {
        return Err(anyhow!("Nincs elküldhető ChatGPT üzenet."));
    }
    Ok(json!({"model": request.model, "input": input, "store": false, "stream": true}))
}

pub async fn stream_chat(request: ChatRequest, sender: UnboundedSender<ProviderEvent>) {
    let panel_id = request.panel_id;
    match run_chat(&request, &sender).await {
        Ok(()) => {
            let _ = sender.send(ProviderEvent::Finished { panel_id });
        }
        Err(error) => {
            let _ = sender.send(ProviderEvent::Failed {
                panel_id,
                message: error.to_string(),
            });
        }
    }
}

async fn run_chat(request: &ChatRequest, sender: &UnboundedSender<ProviderEvent>) -> Result<()> {
    let body = payload(request)?;
    let token = crate::auth::access_token().await?;
    let response = send(
        client()?
            .post(format!("{API}/responses"))
            .bearer_auth(token)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&body),
    )
    .await?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if !content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .eq_ignore_ascii_case("text/event-stream")
    {
        return Err(anyhow!("ChatGPT nem SSE választ küldött."));
    }
    consume_stream(response.bytes_stream(), request.panel_id, sender).await
}

async fn send(builder: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    tokio::time::timeout(INACTIVITY, builder.send())
        .await
        .map_err(|_| anyhow!("ChatGPT kapcsolódási időtúllépés."))?
        .map_err(|_| anyhow!("ChatGPT hálózati vagy TLS hiba."))
}

pub async fn list_models() -> Result<Vec<ModelInfo>> {
    let token = crate::auth::access_token().await?;
    let response = send(client()?.get(format!("{API}/models")).bearer_auth(token)).await?;
    if !response.status().is_success() {
        return Err(response_error(response).await);
    }
    let bytes = read_bounded(response, MAX_INPUT).await?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow!("ChatGPT modellkatalógus hibás JSON."))?;
    parse_models(&value)
}

fn parse_models(value: &Value) -> Result<Vec<ModelInfo>> {
    let models = value
        .get("models")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("ChatGPT modellkatalógus: hiányzó models lista."))?;
    let mut result = Vec::new();
    for model in models {
        if model.get("visibility").and_then(Value::as_str) != Some("list") {
            continue;
        }
        let (Some(id), Some(name)) = (
            model.get("slug").and_then(Value::as_str),
            model.get("display_name").and_then(Value::as_str),
        ) else {
            continue;
        };
        if id.is_empty()
            || id.len() > 160
            || name.is_empty()
            || name.len() > 320
            || id.chars().chain(name.chars()).any(char::is_control)
            || !id
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || "-_.:/".contains(character))
        {
            continue;
        }
        if result.iter().any(|entry: &ModelInfo| entry.id == id) {
            continue;
        }
        result.push(ModelInfo {
            id: id.into(),
            name: name.into(),
        });
        if result.len() == 256 {
            break;
        }
    }
    Ok(result)
}

async fn read_bounded(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = tokio::time::timeout(INACTIVITY, stream.next())
        .await
        .map_err(|_| anyhow!("ChatGPT válasz időtúllépés."))?
    {
        let chunk = chunk.map_err(|_| anyhow!("ChatGPT válasz hálózati hiba."))?;
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(anyhow!(
                "ChatGPT válasz túllépte a biztonságos méretkorlátot."
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

async fn response_error(response: reqwest::Response) -> anyhow::Error {
    let status = response.status().as_u16();
    let request_id = response
        .headers()
        .get("x-request-id")
        .and_then(|value| value.to_str().ok())
        .filter(|value| {
            value.len() <= 128
                && value.starts_with("req_")
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        })
        .map(str::to_owned);
    let body = read_bounded(response, 64 * 1024)
        .await
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    let code = body
        .as_ref()
        .and_then(|value| value.pointer("/error/code"))
        .and_then(Value::as_str);
    let shape = match body.as_ref() {
        Some(value) if value.get("error").is_some() => "error",
        Some(value) if value.get("detail").is_some() => "detail",
        _ => "ismeretlen válaszforma",
    };
    let explanation = known_error(code).unwrap_or(match status {
        401 => "A ChatGPT azonosítás vagy előfizetés-használati engedély nem elfogadott. Ellenőrizd a fiókot és engedélyeket.",
        403 => "ChatGPT hozzáférési vagy régiós korlátozás. Ellenőrizd a fiókot és integrációt.",
        429 => "ChatGPT kéréskorlát; próbáld később.",
        503 => "ChatGPT közvetlen útvonal átmenetileg nem elérhető vagy nem engedélyezett. Próbáld később.",
        _ => "ChatGPT kérés sikertelen.",
    });
    let request = request_id
        .map(|id| format!("; request_id={id}"))
        .unwrap_or_default();
    // Never display diagnostic body text or arbitrary provider codes, which may echo credentials.
    anyhow!("ChatGPT HTTP {status} ({shape}{request}): {explanation}")
}

fn known_error(code: Option<&str>) -> Option<&'static str> {
    match code? {
        "subscription_sharing_usage_limit_exceeded" => Some(
            "subscription_sharing_usage_limit_exceeded: ChatGPT előfizetés-használati korlát elérve. Állítsd le az új kéréseket; részletek: https://chatgpt.com/settings/usage",
        ),
        "subscription_sharing_usage_unavailable" => Some(
            "subscription_sharing_usage_unavailable: az előfizetés használhatósága átmenetileg nem ellenőrizhető. Próbáld később; bejelentkezés megmarad.",
        ),
        "subscription_sharing_user_not_eligible" => Some(
            "subscription_sharing_user_not_eligible: a kiválasztott fiók vagy munkaterület nem jogosult előfizetés-használatra. Ismételt bejelentkezés nem oldja fel.",
        ),
        "subscription_sharing_unsupported_capability" => Some(
            "subscription_sharing_unsupported_capability: nem támogatott bemenet, modell vagy képesség. Módosítsd a kérést újrapróbálás előtt.",
        ),
        "subscription_sharing_route_not_supported" => Some(
            "subscription_sharing_route_not_supported: az integráció számára ez az útvonal nem engedélyezett.",
        ),
        "subscription_sharing_invalid_user" => Some(
            "subscription_sharing_invalid_user: az előfizetői azonosítás nem ellenőrizhető. Ellenőrizd a kiválasztott fiókot.",
        ),
        "subscription_sharing_user_unavailable" => Some(
            "subscription_sharing_user_unavailable: a fiók vagy munkaterület átmenetileg nem elérhető. Próbáld később.",
        ),
        "chatpass_v2_scope_not_authorized" | "chatpass_v2_invalid_authorization_context" => Some(
            "A ChatGPT engedély nem jogosít erre a műveletre. Ellenőrizd az integráció és a jóváhagyás beállításait.",
        ),
        _ => None,
    }
}

#[derive(Default)]
struct ResponseState {
    received_bytes: usize,
    output_bytes: usize,
}

impl ResponseState {
    fn consume(
        &mut self,
        event: SseEvent,
        panel_id: u64,
        sender: &UnboundedSender<ProviderEvent>,
    ) -> Result<bool> {
        let value: Value = serde_json::from_str(&event.data)
            .map_err(|_| anyhow!("ChatGPT stream hibás JSON esemény."))?;
        let kind = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or(&event.event_type);
        if !event.event_type.is_empty() && event.event_type != "message" && event.event_type != kind
        {
            return Err(anyhow!("ChatGPT stream eltérő eseménytípusok."));
        }
        if kind == "error"
            || kind == "response.error"
            || kind == "response.failed"
            || value.get("error").is_some_and(|error| !error.is_null())
        {
            let code = value
                .pointer("/response/error/code")
                .or_else(|| value.pointer("/error/code"))
                .or_else(|| value.get("code"))
                .and_then(Value::as_str);
            return Err(anyhow!(
                "{}",
                known_error(code).unwrap_or("ChatGPT szolgáltatói hiba; a válasz hiányos lehet.")
            ));
        }
        match kind {
            "response.output_text.delta" | "response.refusal.delta" => {
                let text = value
                    .get("delta")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("ChatGPT stream hibás szövegrészlet."))?;
                if text.len() > MAX_OUTPUT.saturating_sub(self.output_bytes) {
                    return Err(anyhow!(
                        "ChatGPT válasz túllépte az 1 MiB korlátot; a válasz hiányos."
                    ));
                }
                self.output_bytes += text.len();
                if !text.is_empty() {
                    sender
                        .send(ProviderEvent::Delta {
                            panel_id,
                            text: text.into(),
                        })
                        .map_err(|_| anyhow!("Beszélgetés lezárva."))?;
                }
            }
            "response.incomplete" => {
                return Err(anyhow!(
                    "ChatGPT válasz hiányos; a modell nem fejezte be a választ."
                ));
            }
            "response.cancelled" => {
                return Err(anyhow!("ChatGPT válasz megszakítva; a válasz hiányos."));
            }
            "response.completed" => {
                let response = value
                    .get("response")
                    .ok_or_else(|| anyhow!("ChatGPT lezáró eseményből hiányzik a response."))?;
                if response.get("status").and_then(Value::as_str) != Some("completed")
                    || response.get("error").is_some_and(|error| !error.is_null())
                {
                    return Err(anyhow!(
                        "ChatGPT lezáró esemény sikertelen vagy hiányos állapotot tartalmaz."
                    ));
                }
                if response
                    .get("output")
                    .and_then(Value::as_array)
                    .is_some_and(|items| items.iter().any(unsupported_item))
                {
                    return Err(anyhow!(
                        "ChatGPT eszközhívást küldött; eszközvégrehajtás nem támogatott."
                    ));
                }
                if let (Some(prompt_tokens), Some(completion_tokens)) = (
                    response
                        .pointer("/usage/input_tokens")
                        .and_then(Value::as_u64),
                    response
                        .pointer("/usage/output_tokens")
                        .and_then(Value::as_u64),
                ) {
                    sender
                        .send(ProviderEvent::Usage {
                            panel_id,
                            usage: Usage {
                                prompt_tokens,
                                completion_tokens,
                            },
                        })
                        .map_err(|_| anyhow!("Beszélgetés lezárva."))?;
                }
                return Ok(true);
            }
            "response.output_item.added" | "response.output_item.done"
                if value.get("item").is_some_and(unsupported_item) =>
            {
                return Err(anyhow!(
                    "ChatGPT eszközhívást küldött; eszközvégrehajtás nem támogatott."
                ));
            }
            _ => {}
        }
        Ok(false)
    }
}

fn unsupported_item(item: &Value) -> bool {
    item.get("type")
        .and_then(Value::as_str)
        .is_some_and(|kind| kind != "message" && kind != "reasoning")
}

async fn consume_stream<S, B, E>(
    mut stream: S,
    panel_id: u64,
    sender: &UnboundedSender<ProviderEvent>,
) -> Result<()>
where
    S: Stream<Item = std::result::Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
{
    let mut parser = SseParser::default();
    let mut state = ResponseState::default();
    loop {
        let chunk = tokio::time::timeout(INACTIVITY, stream.next())
            .await
            .map_err(|_| anyhow!("ChatGPT stream: 90 másodperce nincs adat."))?
            .ok_or_else(|| {
                anyhow!("ChatGPT stream megszakadt response.completed előtt; a válasz hiányos.")
            })?
            .map_err(|_| anyhow!("ChatGPT stream hálózati hiba."))?;
        if chunk.as_ref().len() > MAX_STREAM.saturating_sub(state.received_bytes) {
            return Err(anyhow!(
                "ChatGPT stream túllépte a 8 MiB adatkorlátot; a válasz hiányos."
            ));
        }
        state.received_bytes += chunk.as_ref().len();
        for event in parser
            .feed(chunk.as_ref())
            .map_err(|_| anyhow!("ChatGPT stream hibás vagy túl nagy SSE esemény."))?
        {
            if state.consume(event, panel_id, sender)? {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::Message;

    #[test]
    fn payload_uses_only_supported_fields_and_real_history() {
        let request = ChatRequest {
            panel_id: 1,
            model: "account-model".into(),
            messages: vec![
                Message {
                    role: Role::System,
                    content: "local".into(),
                },
                Message {
                    role: Role::User,
                    content: "hello".into(),
                },
                Message {
                    role: Role::Assistant,
                    content: "answer".into(),
                },
                Message {
                    role: Role::Assistant,
                    content: " \n".into(),
                },
            ],
        };
        let body = payload(&request).unwrap();
        assert_eq!(body.as_object().unwrap().len(), 4);
        assert_eq!(body["store"], false);
        assert_eq!(body["stream"], true);
        assert_eq!(
            body["input"],
            json!([{"role":"user","content":"hello"},{"role":"assistant","content":"answer"}])
        );
    }

    #[test]
    fn models_preserve_server_order_filter_visibility_and_bound_results() {
        let models = parse_models(&json!({"models":[
            {"slug":"second","display_name":"Second","visibility":"list"},
            {"slug":"hidden","display_name":"Hidden","visibility":"hidden"},
            {"slug":"first","display_name":"First","visibility":"list"},
            {"slug":"bad","display_name":"\u{1b}[31m","visibility":"list"}
        ]}))
        .unwrap();
        assert_eq!(
            models
                .iter()
                .map(|model| model.id.as_str())
                .collect::<Vec<_>>(),
            vec!["second", "first"]
        );
        assert_eq!(models[0].name, "Second");
        let list: Vec<_> = (0..300).map(|index| json!({"slug":format!("model-{index}"),"display_name":"Name","visibility":"list"})).collect();
        assert_eq!(parse_models(&json!({"models":list})).unwrap().len(), 256);
    }

    #[tokio::test]
    async fn response_success_handles_utf8_byte_chunks_and_terminal_usage() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let body = concat!(
            ": heartbeat\r\n\r\n",
            "event: response.output_text.delta\r\ndata: {\"type\":\"response.output_text.delta\",\"delta\":\"árvíz 🦀\"}\r\n\r\n",
            "data: {\"type\":\"response.refusal.delta\",\"delta\":\"refusal\"}\n\n",
            "event: response.completed\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"usage\":{\"input_tokens\":8,\"output_tokens\":4}}}\n\n"
        );
        let stream = futures_util::stream::iter(body.as_bytes().chunks(1).map(Ok::<_, ()>));
        consume_stream(stream, 9, &sender).await.unwrap();
        assert!(
            matches!(receiver.try_recv().unwrap(), ProviderEvent::Delta { panel_id:9, text } if text == "árvíz 🦀")
        );
        assert!(
            matches!(receiver.try_recv().unwrap(), ProviderEvent::Delta { text, .. } if text == "refusal")
        );
        assert!(matches!(
            receiver.try_recv().unwrap(),
            ProviderEvent::Usage {
                usage: Usage {
                    prompt_tokens: 8,
                    completion_tokens: 4
                },
                ..
            }
        ));
    }

    #[tokio::test]
    async fn interrupted_incomplete_failed_and_wrong_terminal_status_never_succeed() {
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        for body in [
            "",
            "data: [DONE]\n\n",
            "data: {\"type\":\"response.incomplete\"}\n\n",
            "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"secret-token\"}}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"status\":\"incomplete\"}}\n\n",
        ] {
            let stream = futures_util::stream::iter(vec![Ok::<_, ()>(body.as_bytes())]);
            let error = consume_stream(stream, 1, &sender)
                .await
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret-token"));
        }
    }

    #[tokio::test]
    async fn subscription_failures_remain_actionable_after_partial_output() {
        for code in [
            "subscription_sharing_usage_limit_exceeded",
            "subscription_sharing_usage_unavailable",
        ] {
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let body = format!(
                "data: {{\"type\":\"response.output_text.delta\",\"delta\":\"partial\"}}\n\ndata: {{\"type\":\"response.failed\",\"response\":{{\"error\":{{\"code\":\"{code}\",\"message\":\"secret-token\"}}}}}}\n\n"
            );
            let stream = futures_util::stream::iter(vec![Ok::<_, ()>(body.as_bytes())]);
            let error = consume_stream(stream, 1, &sender)
                .await
                .unwrap_err()
                .to_string();
            assert!(error.contains(code));
            assert!(!error.contains("secret-token"));
            assert!(matches!(
                receiver.try_recv().unwrap(),
                ProviderEvent::Delta { .. }
            ));
        }
    }

    #[test]
    fn input_and_aggregate_output_are_bounded() {
        let request = ChatRequest {
            panel_id: 1,
            model: "model".into(),
            messages: vec![Message {
                role: Role::User,
                content: "x".repeat(MAX_INPUT + 1),
            }],
        };
        assert!(payload(&request).is_err());
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut state = ResponseState::default();
        let data =
            json!({"type":"response.output_text.delta","delta":"x".repeat(65536)}).to_string();
        for _ in 0..16 {
            state
                .consume(
                    SseEvent {
                        data: data.clone(),
                        event_type: String::new(),
                    },
                    1,
                    &sender,
                )
                .unwrap();
            receiver.try_recv().unwrap();
        }
        assert!(
            state
                .consume(
                    SseEvent {
                        data: json!({"type":"response.refusal.delta","delta":"á"}).to_string(),
                        event_type: String::new()
                    },
                    1,
                    &sender
                )
                .is_err()
        );
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn inbound_comment_flood_is_bounded_without_deltas() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut comment = vec![b'x'; 65536];
        comment[0] = b':';
        *comment.last_mut().unwrap() = b'\n';
        let stream = futures_util::stream::iter((0..129).map(|_| Ok::<_, ()>(comment.as_slice())));
        assert!(
            consume_stream(stream, 1, &sender)
                .await
                .unwrap_err()
                .to_string()
                .contains("8 MiB")
        );
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn explicit_error_and_unrequested_tools_fail_without_leaking_payloads() {
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        for data in [
            r#"{"type":"error","code":"secret-token","message":"secret-token"}"#,
            r#"{"type":"response.output_item.added","item":{"type":"function_call","arguments":"secret-token"}}"#,
            r#"{"type":"response.completed","response":{"status":"completed","output":[{"type":"function_call"}]}}"#,
        ] {
            let error = ResponseState::default()
                .consume(
                    SseEvent {
                        data: data.into(),
                        event_type: String::new(),
                    },
                    1,
                    &sender,
                )
                .unwrap_err();
            assert!(!error.to_string().contains("secret-token"));
        }
    }
}
