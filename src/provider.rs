use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

use crate::app::{Message, Role, Usage};

const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";
const INACTIVITY: Duration = Duration::from_secs(90);
const MAX_EVENT_BYTES: usize = 1024 * 1024;
const MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const MAX_STREAM_BYTES: usize = 8 * 1024 * 1024;

pub struct ChatRequest {
    pub panel_id: u64,
    pub model: String,
    pub messages: Vec<Message>,
}

#[derive(Debug)]
pub enum ProviderEvent {
    PreparedInput {
        panel_id: u64,
        content: String,
    },
    Activity {
        panel_id: u64,
        text: String,
    },
    Review {
        panel_id: u64,
        edit: crate::workspace_tools::EditProposal,
        reply: tokio::sync::oneshot::Sender<bool>,
    },
    Delta {
        panel_id: u64,
        text: String,
    },
    Usage {
        panel_id: u64,
        usage: Usage,
    },
    Finished {
        panel_id: u64,
    },
    Failed {
        panel_id: u64,
        message: String,
    },
}

fn payload(request: &ChatRequest) -> Value {
    let messages: Vec<_> = request
        .messages
        .iter()
        .filter(|message| message.role != Role::System)
        .filter(|message| message.role != Role::Assistant || !message.content.trim().is_empty())
        .map(|message| json!({"role": message.role, "content": message.content}))
        .collect();
    json!({
        "model": request.model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
        "max_tokens": 2048
    })
}

pub async fn stream_chat(
    request: ChatRequest,
    api_key: String,
    sender: UnboundedSender<ProviderEvent>,
) {
    let panel_id = request.panel_id;
    match run_stream(&request, api_key, &sender).await {
        Ok(()) => {
            let _ = sender.send(ProviderEvent::Finished { panel_id });
        }
        Err(message) => {
            let _ = sender.send(ProviderEvent::Failed { panel_id, message });
        }
    }
}

async fn run_stream(
    request: &ChatRequest,
    api_key: String,
    sender: &UnboundedSender<ProviderEvent>,
) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return Err("Hiányzó OPENROUTER_API_KEY.".into());
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        // Do not forward credentials through redirected endpoints.
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "HTTP kliens létrehozása sikertelen.")?;
    let response = tokio::time::timeout(
        INACTIVITY,
        client
            .post(ENDPOINT)
            .bearer_auth(api_key)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .json(&payload(request))
            .send(),
    )
    .await
    .map_err(|_| "OpenRouter kapcsolódási időtúllépés.")?
    .map_err(|error| {
        if error.is_timeout() {
            "OpenRouter kapcsolódási időtúllépés."
        } else {
            "OpenRouter hálózati vagy TLS hiba."
        }
    })?;
    if !response.status().is_success() {
        // Server bodies may echo credentials or prompts. Never display them.
        return Err(http_error(response.status().as_u16()));
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
        return Err("OpenRouter nem SSE választ küldött.".into());
    }
    consume_stream(response.bytes_stream(), request.panel_id, sender).await
}

async fn consume_stream<S, B, E>(
    mut stream: S,
    panel_id: u64,
    sender: &UnboundedSender<ProviderEvent>,
) -> Result<(), String>
where
    S: futures_util::Stream<Item = Result<B, E>> + Unpin,
    B: AsRef<[u8]>,
{
    let mut parser = SseParser::default();
    let mut state = CompletionState::default();
    loop {
        let chunk = tokio::time::timeout(INACTIVITY, stream.next())
            .await
            .map_err(|_| "OpenRouter stream: 90 másodperce nincs adat.")?
            .ok_or("OpenRouter stream megszakadt a [DONE] lezárás előtt.")?
            .map_err(|_| "OpenRouter stream hálózati hiba.")?;
        state.record_inbound(chunk.as_ref().len())?;
        for event in parser.feed(chunk.as_ref())? {
            if state.consume(event, panel_id, sender)? {
                return Ok(());
            }
        }
    }
}

pub(crate) fn http_error(status: u16) -> String {
    let explanation = match status {
        400 => "Érvénytelen kérés vagy modell.",
        401 | 403 => "API-kulcs vagy hozzáférés hibás.",
        402 => "Nincs elegendő OpenRouter egyenleg.",
        404 => {
            "A modell nem található, vagy nincs eszközhívást támogató végpontja. Válassz tools-képes modellt."
        }
        429 => "Kéréskorlát elérve; próbáld később.",
        500..=599 => "OpenRouter vagy modellszolgáltató hiba.",
        _ => "Kérés sikertelen.",
    };
    format!("OpenRouter HTTP {status}: {explanation}")
}

#[derive(Default)]
pub(crate) struct SseParser {
    line: Vec<u8>,
    data: String,
    event_type: String,
    after_cr: bool,
    started: bool,
}

pub(crate) struct SseEvent {
    pub(crate) data: String,
    pub(crate) event_type: String,
}

impl SseParser {
    // Buffer bytes until complete lines, preserving split UTF-8 code points.
    // CR, LF, and CRLF are valid SSE line endings, even across HTTP chunks.
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<Vec<SseEvent>, String> {
        let mut events = Vec::new();
        for &byte in bytes {
            if self.after_cr && byte == b'\n' {
                self.after_cr = false;
                continue;
            }
            self.after_cr = byte == b'\r';
            if byte == b'\r' || byte == b'\n' {
                self.complete_line(&mut events)?;
            } else {
                self.line.push(byte);
                if self.line.len() + self.data.len() > MAX_EVENT_BYTES {
                    return Err("OpenRouter SSE esemény túl nagy.".into());
                }
            }
        }
        Ok(events)
    }

    fn complete_line(&mut self, events: &mut Vec<SseEvent>) -> Result<(), String> {
        let raw = std::mem::take(&mut self.line);
        let line = std::str::from_utf8(&raw).map_err(|_| "OpenRouter stream hibás UTF-8 adat.")?;
        let line = if !self.started {
            self.started = true;
            line.trim_start_matches('\u{feff}')
        } else {
            line
        };
        if line.is_empty() {
            if !self.data.is_empty() || self.event_type == "error" {
                self.data.pop(); // Remove the newline appended to the final data field.
                events.push(SseEvent {
                    data: std::mem::take(&mut self.data),
                    event_type: std::mem::take(&mut self.event_type),
                });
            }
            self.event_type.clear();
        } else if !line.starts_with(':') {
            let (field, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            match field {
                "data" => {
                    self.data.push_str(value);
                    self.data.push('\n');
                }
                "event" => self.event_type = value.into(),
                _ => {}
            }
            if self.data.len() > MAX_EVENT_BYTES {
                return Err("OpenRouter SSE esemény túl nagy.".into());
            }
        }
        Ok(())
    }
}

#[derive(Default)]
struct CompletionState {
    finished_choice: bool,
    received_bytes: usize,
    output_bytes: usize,
}

impl CompletionState {
    fn record_inbound(&mut self, count: usize) -> Result<(), String> {
        if count > MAX_STREAM_BYTES.saturating_sub(self.received_bytes) {
            return Err(
                "OpenRouter stream túllépte a 8 MiB adatkorlátot; a válasz hiányos.".into(),
            );
        }
        self.received_bytes += count;
        Ok(())
    }

    fn emit_text(
        &mut self,
        text: &str,
        panel_id: u64,
        sender: &UnboundedSender<ProviderEvent>,
    ) -> Result<(), String> {
        if text.len() > MAX_OUTPUT_BYTES.saturating_sub(self.output_bytes) {
            return Err(
                "A válasz túllépte az 1 MiB szövegkorlátot; a megjelenített válasz hiányos.".into(),
            );
        }
        self.output_bytes += text.len();
        sender
            .send(ProviderEvent::Delta {
                panel_id,
                text: text.into(),
            })
            .map_err(|_| "Beszélgetés lezárva.".into())
    }

    fn consume(
        &mut self,
        event: SseEvent,
        panel_id: u64,
        sender: &UnboundedSender<ProviderEvent>,
    ) -> Result<bool, String> {
        if event.event_type == "error" {
            return Err("OpenRouter stream szolgáltatói hiba.".into());
        }
        if event.data == "[DONE]" {
            return if self.finished_choice {
                Ok(true)
            } else {
                Err("OpenRouter stream lezárult érvényes finish_reason nélkül.".into())
            };
        }
        let chunk: Value = serde_json::from_str(&event.data)
            .map_err(|_| "OpenRouter stream hibás JSON esemény.")?;
        if chunk.get("error").is_some_and(|error| !error.is_null()) {
            return Err("OpenRouter stream szolgáltatói hiba.".into());
        }
        if let Some(choices) = chunk.get("choices").and_then(Value::as_array) {
            for choice in choices {
                if choice
                    .get("index")
                    .and_then(Value::as_u64)
                    .is_some_and(|index| index != 0)
                {
                    continue;
                }
                if choice.get("error").is_some_and(|error| !error.is_null()) {
                    return Err("OpenRouter stream szolgáltatói hiba.".into());
                }
                if let Some(text) = choice
                    .pointer("/delta/content")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    self.emit_text(text, panel_id, sender)?;
                }
                if let Some(text) = choice
                    .pointer("/delta/refusal")
                    .and_then(Value::as_str)
                    .filter(|text| !text.is_empty())
                {
                    self.emit_text(text, panel_id, sender)?;
                }
                if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    match reason {
                        "stop" => self.finished_choice = true,
                        "length" => return Err("A modell elérte a tokenkorlátot; a válasz hiányos.".into()),
                        "content_filter" => return Err("A szolgáltató tartalomszűrője leállította a választ; a válasz hiányos lehet.".into()),
                        "tool_calls" => return Err("A modell eszközhívást kért; eszközvégrehajtás nem támogatott.".into()),
                        _ => return Err("OpenRouter stream hibás vagy sikertelen finish_reason.".into()),
                    }
                }
            }
        }
        // Do not invent token counts when providers return null, negative or fractional values.
        if let Some(usage) = chunk.get("usage")
            && let (Some(prompt_tokens), Some(completion_tokens)) = (
                usage.get("prompt_tokens").and_then(Value::as_u64),
                usage.get("completion_tokens").and_then(Value::as_u64),
            )
        {
            sender
                .send(ProviderEvent::Usage {
                    panel_id,
                    usage: Usage {
                        prompt_tokens,
                        completion_tokens,
                    },
                })
                .map_err(|_| "Beszélgetés lezárva.")?;
        }
        Ok(false)
    }
}

pub async fn demo_chat(request: ChatRequest, sender: UnboundedSender<ProviderEvent>) {
    let panel_id = request.panel_id;
    let answer = "DEMO — szimulált válasz, nincs modellhívás.\n\nEz a panel saját beszélgetést és modellt tart fenn. Küldj üzeneteket, nyiss másik panelt, majd hasonlítsd össze a válaszokat. A demó szövegrészletekkel mutatja a streamelést. Nem olvas fájlokat, nem futtat parancsokat, és nem hajt végre eszközműveleteket.\n\nValódi válaszhoz indítsd az alkalmazást demómód nélkül, beállított OPENROUTER_API_KEY környezeti változóval.";
    for word in answer.split_inclusive(' ') {
        if sender
            .send(ProviderEvent::Delta {
                panel_id,
                text: word.into(),
            })
            .is_err()
        {
            return;
        }
        tokio::time::sleep(Duration::from_millis(28)).await;
    }
    let _ = sender.send(ProviderEvent::Finished { panel_id });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(data: &str) -> SseEvent {
        SseEvent {
            data: data.into(),
            event_type: String::new(),
        }
    }

    #[test]
    fn payload_filters_local_notices_and_limits_output() {
        let request = ChatRequest {
            panel_id: 1,
            model: "test/model".into(),
            messages: vec![
                Message {
                    role: Role::System,
                    content: "local notice".into(),
                },
                Message {
                    role: Role::User,
                    content: "question".into(),
                },
                Message {
                    role: Role::Assistant,
                    content: "answer".into(),
                },
                Message {
                    role: Role::Assistant,
                    content: String::new(),
                },
                Message {
                    role: Role::Assistant,
                    content: " \n\t".into(),
                },
            ],
        };
        let body = payload(&request);
        assert_eq!(body["messages"].as_array().unwrap().len(), 2);
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][1]["role"], "assistant");
        assert_eq!(body["max_tokens"], 2048);
        assert_eq!(body["stream_options"]["include_usage"], true);
        assert!(body.get("tools").is_none());
    }

    #[test]
    fn sse_handles_all_byte_boundaries_utf8_comments_crlf_and_multiline() {
        let input = "\u{feff}: heartbeat\r\nevent: message\r\ndata: {\r\ndata: \"text\":\"árvíz 🦀\"}\r\n\r\n";
        for boundary in 0..=input.len() {
            let mut parser = SseParser::default();
            let mut events = parser.feed(&input.as_bytes()[..boundary]).unwrap();
            events.extend(parser.feed(&input.as_bytes()[boundary..]).unwrap());
            assert_eq!(events.len(), 1);
            assert_eq!(events[0].data, "{\n\"text\":\"árvíz 🦀\"}");
        }
        let mut parser = SseParser::default();
        let mut events = Vec::new();
        for byte in input.as_bytes() {
            events.extend(parser.feed(&[*byte]).unwrap());
        }
        assert_eq!(events.len(), 1);
    }

    #[test]
    fn sse_supports_cr_lf_empty_data_unknown_fields_and_error_event() {
        let events = SseParser::default()
            .feed(b": comment\r\rdata: x\r\rid: ignored\ndata:\n\nevent: error\n\n")
            .unwrap();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].data, "x");
        assert_eq!(events[1].data, "");
        assert_eq!(events[2].event_type, "error");
    }

    #[test]
    fn invalid_utf8_and_oversized_events_fail() {
        assert!(SseParser::default().feed(b"data: \xff\n\n").is_err());
        assert!(
            SseParser::default()
                .feed(&vec![b'x'; MAX_EVENT_BYTES + 1])
                .is_err()
        );
    }

    #[test]
    fn done_requires_valid_finish_and_usage_may_repeat_finish() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut state = CompletionState::default();
        assert!(state.consume(event("[DONE]"), 1, &sender).is_err());
        assert!(!state.consume(event(r#"{"choices":[{"index":0,"delta":{"content":"hello"},"finish_reason":"stop"}]}"#), 1, &sender).unwrap());
        assert!(!state.consume(event(r#"{"choices":[{"index":0,"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2}}"#), 1, &sender).unwrap());
        assert!(state.consume(event("[DONE]"), 1, &sender).unwrap());
        assert!(matches!(
            receiver.try_recv().unwrap(),
            ProviderEvent::Delta { panel_id: 1, .. }
        ));
        assert!(matches!(
            receiver.try_recv().unwrap(),
            ProviderEvent::Usage {
                usage: Usage {
                    prompt_tokens: 3,
                    completion_tokens: 2
                },
                ..
            }
        ));
    }

    #[test]
    fn midstream_errors_fail_even_after_finish_without_exposing_remote_text() {
        let (sender, _) = tokio::sync::mpsc::unbounded_channel();
        let mut state = CompletionState {
            finished_choice: true,
            ..CompletionState::default()
        };
        for data in [
            r#"{"error":{"message":"secret-key"}}"#,
            r#"{"choices":[{"finish_reason":"error"}]}"#,
            r#"{"choices":[{"finish_reason":"tool_calls"}]}"#,
            "invalid",
        ] {
            let error = state.consume(event(data), 1, &sender).unwrap_err();
            assert!(!error.contains("secret-key"));
        }
    }

    #[test]
    fn unsupported_usage_is_ignored() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        for count in ["null", "-1", "1.5", "\"4\""] {
            let data = format!(
                r#"{{"choices":[],"usage":{{"prompt_tokens":{count},"completion_tokens":1}}}}"#
            );
            CompletionState::default()
                .consume(event(&data), 1, &sender)
                .unwrap();
        }
        assert!(receiver.try_recv().is_err());
    }

    #[tokio::test]
    async fn premature_eof_fails_even_after_choice_finish() {
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        for body in [
            "",
            "data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]",
        ] {
            let stream = futures_util::stream::iter(vec![Ok::<_, ()>(body.as_bytes())]);
            assert!(
                consume_stream(stream, 1, &sender)
                    .await
                    .unwrap_err()
                    .contains("[DONE]")
            );
        }
    }

    #[tokio::test]
    async fn stream_success_and_midstream_failure() {
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let success = b"data: {\"choices\":[{\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n";
        let stream = futures_util::stream::iter(success.chunks(1).map(Ok::<_, ()>));
        assert!(consume_stream(stream, 1, &sender).await.is_ok());
        let failed = b"data: {\"error\":{\"message\":\"secret-key\"}}\n\ndata: [DONE]\n\n";
        let stream = futures_util::stream::iter(vec![Ok::<_, ()>(failed.as_slice())]);
        let error = consume_stream(stream, 1, &sender).await.unwrap_err();
        assert!(!error.contains("secret-key"));
    }

    #[tokio::test]
    async fn missing_key_emits_only_failed() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        stream_chat(
            ChatRequest {
                panel_id: 42,
                model: "test".into(),
                messages: vec![],
            },
            String::new(),
            sender,
        )
        .await;
        assert!(matches!(
            receiver.recv().await,
            Some(ProviderEvent::Failed { panel_id: 42, .. })
        ));
        assert!(receiver.recv().await.is_none());
    }

    #[tokio::test]
    async fn truncated_and_filtered_endings_preserve_partial_text_and_fail() {
        for (reason, expected) in [
            ("length", "tokenkorlátot"),
            ("content_filter", "tartalomszűrője"),
        ] {
            let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
            let body = format!(
                "data: {{\"choices\":[{{\"delta\":{{\"content\":\"részleges\"}},\"finish_reason\":\"{reason}\"}}]}}\n\ndata: [DONE]\n\n"
            );
            let stream = futures_util::stream::iter(vec![Ok::<_, ()>(body.as_bytes())]);
            let error = consume_stream(stream, 7, &sender).await.unwrap_err();
            assert!(error.contains(expected));
            assert!(
                matches!(receiver.try_recv().unwrap(), ProviderEvent::Delta { panel_id: 7, text } if text == "részleges")
            );
            assert!(receiver.try_recv().is_err());
        }
    }

    #[test]
    fn aggregate_text_limit_counts_content_and_refusal_in_utf8_bytes() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut state = CompletionState::default();
        let text = "x".repeat(64 * 1024);
        let frame = json!({"choices": [{"delta": {"content": text}}]}).to_string();
        for _ in 0..16 {
            assert!(!state.consume(event(&frame), 1, &sender).unwrap());
            assert!(matches!(
                receiver.try_recv().unwrap(),
                ProviderEvent::Delta { .. }
            ));
        }
        assert_eq!(state.output_bytes, MAX_OUTPUT_BYTES);
        let refusal = r#"{"choices":[{"delta":{"refusal":"á"}}]}"#;
        assert!(
            state
                .consume(event(refusal), 1, &sender)
                .unwrap_err()
                .contains("1 MiB")
        );
        assert!(receiver.try_recv().is_err());
        assert_eq!(state.output_bytes, MAX_OUTPUT_BYTES);
    }

    #[tokio::test]
    async fn inbound_flood_limit_includes_comments_without_output() {
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        let mut comment = vec![b'x'; 64 * 1024];
        comment[0] = b':';
        *comment.last_mut().unwrap() = b'\n';
        let stream = futures_util::stream::iter((0..129).map(|_| Ok::<_, ()>(comment.as_slice())));
        assert!(
            consume_stream(stream, 1, &sender)
                .await
                .unwrap_err()
                .contains("8 MiB")
        );
        assert!(receiver.try_recv().is_err());
    }
}
