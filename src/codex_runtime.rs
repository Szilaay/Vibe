//! Local Codex app-server sessions. Only VIBE's own OAuth token enters the child.
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, ChildStdin, ChildStdout, Command},
    runtime::Handle,
    sync::{mpsc, oneshot, watch},
    task::JoinHandle,
};

use crate::{app::Usage, auth, provider::ProviderEvent};

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_FRAME: usize = 4 * 1024 * 1024;
const PROVIDER: &str = "openai_chatgpt_plan";

#[derive(Debug)]
pub enum RuntimeStatus {
    Ready,
    Failed(String),
}

struct Turn {
    model: String,
    effort: crate::app::Effort,
    text: String,
    output: mpsc::UnboundedSender<ProviderEvent>,
    finished: oneshot::Sender<()>,
}

enum SessionCommand {
    Turn(Turn),
    Cancel,
}

pub struct CodexSession {
    commands: mpsc::UnboundedSender<SessionCommand>,
    shutdown: watch::Sender<bool>,
    task: Option<JoinHandle<()>>,
}

impl CodexSession {
    pub fn start(
        runtime: &Handle,
        panel_id: u64,
        cwd: PathBuf,
        model: String,
        effort: crate::app::Effort,
        status: mpsc::UnboundedSender<RuntimeStatus>,
    ) -> Self {
        let (commands, receiver) = mpsc::unbounded_channel();
        let (shutdown, mut stopping) = watch::channel(false);
        let task = runtime.spawn(async move {
            // Dropping the actor also drops kill_on_drop Child and never strands its process.
            tokio::select! {
                _ = stopping.changed() => {},
                result = run_session(panel_id, cwd, model, effort, receiver, &status) => {
                    if let Err(error) = result { let _ = status.send(RuntimeStatus::Failed(error.to_string())); }
                }
            }
        });
        Self {
            commands,
            shutdown,
            task: Some(task),
        }
    }

    pub fn send(
        &self,
        model: String,
        effort: crate::app::Effort,
        text: String,
        output: mpsc::UnboundedSender<ProviderEvent>,
    ) -> Result<oneshot::Receiver<()>> {
        let (finished, receiver) = oneshot::channel();
        self.commands
            .send(SessionCommand::Turn(Turn {
                model,
                effort,
                text,
                output,
                finished,
            }))
            .map_err(|_| anyhow::anyhow!("Codex CLI leállt. Nyiss új Codex CLI panelt."))?;
        Ok(receiver)
    }

    pub fn cancel(&self) {
        let _ = self.commands.send(SessionCommand::Cancel);
    }

    pub fn stop(mut self) -> JoinHandle<()> {
        let _ = self.shutdown.send(true);
        self.task.take().expect("session task")
    }
}

impl Drop for CodexSession {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

async fn run_session(
    panel_id: u64,
    cwd: PathBuf,
    model: String,
    effort: crate::app::Effort,
    mut commands: mpsc::UnboundedReceiver<SessionCommand>,
    status: &mpsc::UnboundedSender<RuntimeStatus>,
) -> Result<()> {
    let home = tempfile::Builder::new()
        .prefix("vibe-codex-")
        .tempdir()
        .context("Codex CLI saját adatkönyvtára nem hozható létre")?;
    let mut token = auth::access_token().await?;
    let mut server = Server::start(&cwd, home.path(), &token).await?;
    let mut thread = server
        .open_thread(&cwd, &model, effort, None)
        .await
        .map_err(|error| anyhow::anyhow!(clean_error(&error.to_string(), &token)))?;
    let _ = status.send(RuntimeStatus::Ready);
    let mut has_turn = false;
    loop {
        let command = if let Some(turn) = server.pending.pop_front() {
            SessionCommand::Turn(turn)
        } else {
            tokio::select! {
                command = commands.recv() => match command { Some(command) => command, None => break },
                incoming = server.rpc.next() => {
                    let incoming = incoming?;
                    server.rpc.reject_server_request(&incoming).await?;
                    continue;
                }
            }
        };
        let SessionCommand::Turn(turn) = command else {
            continue;
        };
        if turn.finished.is_closed() {
            continue;
        }
        let result: Result<()> = async {
            let fresh = auth::access_token().await?;
            if fresh != token {
                let pending = std::mem::take(&mut server.pending);
                server.stop().await;
                token = fresh;
                server = Server::start(&cwd, home.path(), &token).await?;
                server.pending = pending;
                // Codex creates its rollout on the first turn; an untouched thread cannot resume.
                thread = server
                    .open_thread(
                        &cwd,
                        &turn.model,
                        turn.effort,
                        has_turn.then_some(thread.as_str()),
                    )
                    .await?;
            }
            server
                .turn(
                    panel_id,
                    &thread,
                    &turn,
                    &mut commands,
                    &token,
                    &mut has_turn,
                )
                .await
        }
        .await;
        let terminal = match result {
            Ok(()) => ProviderEvent::Finished { panel_id },
            Err(error) => ProviderEvent::Failed {
                panel_id,
                message: clean_error(&error.to_string(), &token),
            },
        };
        let _ = turn.output.send(terminal);
        let _ = turn.finished.send(());
        // Interrupted turns are drained before another turn is accepted.
        if server.child.try_wait()?.is_some() {
            bail!("Codex CLI folyamat leállt.");
        }
    }
    server.stop().await;
    Ok(())
}

struct Server {
    child: Child,
    rpc: Rpc<ChildStdout, ChildStdin>,
    stderr: JoinHandle<()>,
    pending: VecDeque<Turn>,
}

impl Server {
    async fn start(cwd: &Path, home: &Path, token: &str) -> Result<Self> {
        let executable = resolve_executable()?;
        let mut command = Command::new(executable);
        command
            .args(server_arguments())
            .current_dir(cwd)
            .env("CODEX_HOME", home)
            .env("ACCESS_TOKEN", token)
            .env_remove("OPENAI_API_KEY")
            .env_remove("OPENAI_ACCESS_TOKEN")
            .env_remove("CODEX_API_KEY")
            .env_remove("OPENROUTER_API_KEY")
            .env_remove("CODEX_INTERNAL_ORIGINATOR_OVERRIDE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        let mut child = command.spawn().context("Codex CLI nem indítható")?;
        let input = child.stdin.take().context("Codex CLI stdin hiányzik")?;
        let output = child.stdout.take().context("Codex CLI stdout hiányzik")?;
        let mut stderr = child.stderr.take().context("Codex CLI stderr hiányzik")?;
        // Never log child diagnostics: they can contain credentials or source text.
        let stderr = tokio::spawn(async move {
            let _ = tokio::io::copy(&mut stderr, &mut tokio::io::sink()).await;
        });
        let mut server = Self {
            child,
            rpc: Rpc::new(output, input),
            stderr,
            pending: VecDeque::new(),
        };
        server.rpc.call("initialize", json!({"clientInfo":{"name":"vibe","title":"VIBE","version":env!("CARGO_PKG_VERSION")}})).await
            .map_err(|error| anyhow::anyhow!(clean_error(&error.to_string(), token)))?;
        server.rpc.write(json!({"method":"initialized"})).await?;
        Ok(server)
    }

    async fn open_thread(
        &mut self,
        cwd: &Path,
        model: &str,
        effort: crate::app::Effort,
        previous: Option<&str>,
    ) -> Result<String> {
        let mut params = json!({
            "cwd": cwd,
            "model": model,
            "modelProvider": PROVIDER,
            "approvalPolicy": "never",
            "sandbox": "workspace-write",
            "reasoningEffort": effort.reasoning(),
        });
        let method = if let Some(previous) = previous {
            params["threadId"] = json!(previous);
            "thread/resume"
        } else {
            "thread/start"
        };
        let result = self.rpc.call(method, params).await?;
        result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .context("Codex CLI nem adott thread azonosítót")
    }

    async fn turn(
        &mut self,
        panel_id: u64,
        thread: &str,
        turn: &Turn,
        commands: &mut mpsc::UnboundedReceiver<SessionCommand>,
        token: &str,
        has_turn: &mut bool,
    ) -> Result<()> {
        let response = self
            .rpc
            .call(
                "turn/start",
                json!({
                    "threadId": thread,
                    "model": turn.model,
                    "reasoningEffort": turn.effort.reasoning(),
                    "approvalPolicy": "never",
                    "input": [{"type":"text","text":turn.text}],
                }),
            )
            .await?;
        let turn_id = response
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .context("Codex CLI nem adott turn azonosítót")?
            .to_owned();
        *has_turn = true;
        let mut streamed = HashSet::new();
        let mut cancelled = false;
        loop {
            let message = tokio::select! {
                message = self.rpc.next() => message?,
                command = commands.recv() => {
                    match command {
                        Some(SessionCommand::Cancel) if !cancelled => {
                            // Interrupt response may race completion; call preserves notifications.
                            self.rpc.call("turn/interrupt", json!({"threadId":thread,"turnId":turn_id})).await?;
                            cancelled = true;
                        }
                        Some(SessionCommand::Turn(queued)) => {
                            // The UI can send again immediately after Cancel. Drain the old turn
                            // before starting the next one on this same thread and process.
                            self.pending.push_back(queued);
                        }
                        None => bail!("Codex CLI panel bezárva."),
                        _ => {}
                    }
                    continue;
                }
            };
            if self.rpc.reject_server_request(&message).await? {
                let _ = turn.output.send(ProviderEvent::Delta {
                    panel_id,
                    text: "\n[Codex CLI: nem támogatott engedélykérés elutasítva.]\n".into(),
                });
                continue;
            }
            let params = &message["params"];
            if params["threadId"].as_str().is_some_and(|id| id != thread)
                || params["turnId"].as_str().is_some_and(|id| id != turn_id)
            {
                continue;
            }
            match message["method"].as_str() {
                Some("item/agentMessage/delta") if !cancelled => {
                    streamed.insert(params["itemId"].as_str().unwrap_or_default().to_owned());
                    if let Some(text) = params["delta"].as_str() {
                        let _ = turn.output.send(ProviderEvent::Delta {
                            panel_id,
                            text: clean_error(text, token),
                        });
                    }
                }
                Some("item/completed")
                    if !cancelled && params["item"]["type"] == "agentMessage" =>
                {
                    let item = &params["item"];
                    if !streamed.contains(item["id"].as_str().unwrap_or_default())
                        && let Some(text) = item["text"].as_str()
                    {
                        let _ = turn.output.send(ProviderEvent::Delta {
                            panel_id,
                            text: clean_error(text, token),
                        });
                    }
                }
                Some("item/started") if !cancelled => {
                    let text = match params["item"]["type"].as_str() {
                        Some("commandExecution") => Some("\n[Codex CLI: parancs futtatása…]\n"),
                        Some("fileChange") => Some("\n[Codex CLI: fájlok módosítása…]\n"),
                        _ => None,
                    };
                    if let Some(text) = text {
                        let _ = turn.output.send(ProviderEvent::Delta {
                            panel_id,
                            text: text.into(),
                        });
                    }
                }
                Some("thread/tokenUsage/updated") => {
                    let usage = &params["tokenUsage"]["last"];
                    let _ = turn.output.send(ProviderEvent::Usage {
                        panel_id,
                        usage: Usage {
                            prompt_tokens: usage["inputTokens"].as_u64().unwrap_or(0),
                            completion_tokens: usage["outputTokens"].as_u64().unwrap_or(0),
                        },
                    });
                }
                Some("turn/completed") if params["turn"]["id"] == turn_id => {
                    return completed_turn(&params["turn"], token);
                }
                _ => {}
            }
        }
    }

    async fn stop(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        self.stderr.abort();
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stderr.abort();
    }
}

fn completed_turn(turn: &Value, token: &str) -> Result<()> {
    match turn["status"].as_str() {
        Some("completed") => Ok(()),
        Some("interrupted") => bail!("Codex CLI feladat leállítva."),
        Some("failed") => bail!(
            "{}",
            clean_error(
                turn.pointer("/error/message")
                    .and_then(Value::as_str)
                    .unwrap_or("Codex CLI feladat sikertelen."),
                token
            )
        ),
        _ => bail!("Codex CLI ismeretlen befejezési állapotot küldött."),
    }
}

fn clean_error(text: &str, token: &str) -> String {
    // Keep response formatting; remove the known bearer token wherever it appears.
    if token.is_empty() {
        text.to_owned()
    } else {
        text.replace(token, "[redacted]")
    }
}

pub(crate) struct Rpc<R, W> {
    reader: BufReader<R>,
    writer: W,
    serial: u64,
    notifications: VecDeque<Value>,
    frame: Vec<u8>,
}

impl<R: AsyncRead + Unpin, W: AsyncWrite + Unpin> Rpc<R, W> {
    pub(crate) fn new(reader: R, writer: W) -> Self {
        Self {
            reader: BufReader::new(reader),
            writer,
            serial: 0,
            notifications: VecDeque::new(),
            frame: Vec::new(),
        }
    }

    pub(crate) async fn write(&mut self, message: Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(&message)?;
        bytes.push(b'\n');
        self.writer
            .write_all(&bytes)
            .await
            .context("Codex CLI kapcsolat megszakadt")?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn read(&mut self) -> Result<Value> {
        loop {
            let available = self
                .reader
                .fill_buf()
                .await
                .context("Codex CLI olvasási hiba")?;
            ensure!(
                !available.is_empty(),
                "Codex CLI folyamat lezárta a kapcsolatot."
            );
            let length = available
                .iter()
                .position(|byte| *byte == b'\n')
                .map_or(available.len(), |end| end + 1);
            ensure!(
                self.frame.len() + length <= MAX_FRAME,
                "Codex CLI túl nagy protokollüzenetet küldött."
            );
            let complete = available[length - 1] == b'\n';
            self.frame.extend_from_slice(&available[..length]);
            self.reader.consume(length);
            if complete {
                let bytes = std::mem::take(&mut self.frame);
                if bytes.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                return serde_json::from_slice(&bytes)
                    .context("Codex CLI érvénytelen JSON választ küldött");
            }
        }
    }

    pub(crate) async fn next(&mut self) -> Result<Value> {
        match self.notifications.pop_front() {
            Some(message) => Ok(message),
            None => self.read().await,
        }
    }

    pub(crate) async fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.serial += 1;
        let id = self.serial;
        self.write(json!({"id":id,"method":method,"params":params}))
            .await?;
        tokio::time::timeout(RPC_TIMEOUT, async {
            loop {
                let message = self.read().await?;
                if self.reject_server_request(&message).await? {
                    continue;
                }
                if message["id"] == id {
                    if let Some(error) = message.get("error") {
                        bail!(
                            "Codex CLI {method}: {}",
                            error["message"].as_str().unwrap_or("RPC hiba")
                        );
                    }
                    return message
                        .get("result")
                        .cloned()
                        .context("Codex CLI RPC eredmény hiányzik");
                }
                ensure!(
                    self.notifications.len() < 4096,
                    "Codex CLI túl sok várakozó eseményt küldött."
                );
                self.notifications.push_back(message);
            }
        })
        .await
        .context("Codex CLI válaszidő túllépve")?
    }

    pub(crate) async fn reject_server_request(&mut self, message: &Value) -> Result<bool> {
        if message.get("id").is_none() || message.get("method").is_none() {
            return Ok(false);
        }
        let response = match message["method"].as_str() {
            Some("item/commandExecution/requestApproval" | "item/fileChange/requestApproval") => {
                json!({"id":message["id"],"result":{"decision":"decline"}})
            }
            _ => {
                json!({"id":message["id"],"error":{"code":-32601,"message":"VIBE does not support this server request; permission denied."}})
            }
        };
        self.write(response).await?;
        Ok(true)
    }
}

fn server_arguments() -> Vec<String> {
    let mut arguments = vec!["app-server".into(), "--listen".into(), "stdio://".into()];
    for setting in [
        "model_provider=\"openai_chatgpt_plan\"",
        "model_providers.openai_chatgpt_plan.name=\"ChatGPT plan\"",
        "model_providers.openai_chatgpt_plan.base_url=\"https://api.openai.com/v1\"",
        "model_providers.openai_chatgpt_plan.env_key=\"ACCESS_TOKEN\"",
        "model_providers.openai_chatgpt_plan.wire_api=\"responses\"",
        "model_providers.openai_chatgpt_plan.requires_openai_auth=false",
        "model_providers.openai_chatgpt_plan.supports_websockets=false",
        "approval_policy=\"never\"",
        "sandbox_mode=\"workspace-write\"",
        "sandbox_workspace_write.network_access=false",
    ] {
        arguments.push("-c".into());
        arguments.push(setting.into());
    }
    arguments
}

pub(crate) fn resolve_executable() -> Result<PathBuf> {
    let paths = std::env::var_os("PATH").context("PATH hiányzik; telepítsd a Codex CLI-t")?;
    let directories: Vec<_> = std::env::split_paths(&paths).collect();
    for directory in directories {
        let executable = directory.join(if cfg!(windows) { "codex.exe" } else { "codex" });
        if executable.is_file() {
            #[cfg(unix)]
            {
                // npm's Unix executable is a symlink to a JS shim. Prefer its native vendor binary.
                if let Ok(real) = executable.canonicalize()
                    && real.extension().is_some_and(|extension| extension == "js")
                    && let Some(package) = real.parent().and_then(Path::parent)
                    && let Some(binary) = npm_binary(package)
                {
                    return Ok(binary);
                }
            }
            return Ok(executable);
        }
        if let Some(binary) = npm_binary(&directory.join("node_modules/@openai/codex")) {
            return Ok(binary);
        }
    }
    bail!(
        "Codex CLI nem található. Telepítés: npm install -g @openai/codex; majd indítsd újra a VIBE alkalmazást."
    )
}

fn npm_binary(package: &Path) -> Option<PathBuf> {
    let (platform, target) = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => ("win32-x64", "x86_64-pc-windows-msvc"),
        ("windows", "aarch64") => ("win32-arm64", "aarch64-pc-windows-msvc"),
        ("linux", "x86_64") => ("linux-x64", "x86_64-unknown-linux-musl"),
        ("linux", "aarch64") => ("linux-arm64", "aarch64-unknown-linux-musl"),
        ("macos", "x86_64") => ("darwin-x64", "x86_64-apple-darwin"),
        ("macos", "aarch64") => ("darwin-arm64", "aarch64-apple-darwin"),
        _ => return None,
    };
    let suffix =
        PathBuf::from(target)
            .join("bin")
            .join(if cfg!(windows) { "codex.exe" } else { "codex" });
    [
        package
            .join("node_modules/@openai")
            .join(format!("codex-{platform}"))
            .join("vendor"),
        package
            .parent()?
            .join(format!("codex-{platform}"))
            .join("vendor"),
        package.join("vendor"),
    ]
    .into_iter()
    .map(|root| root.join(&suffix))
    .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn rpc_preserves_notifications_and_denies_approval_before_result() {
        let (client, server) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let mut rpc = Rpc::new(read, write);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut read = BufReader::new(read);
            let mut line = String::new();
            read.read_line(&mut line).await.unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap()["method"],
                "turn/start"
            );
            write.write_all(b"{\"method\":\"item/agentMessage/delta\",\"params\":{\"delta\":\"hello\"}}\n{\"id\":\"approval\",\"method\":\"item/commandExecution/requestApproval\"}\n").await.unwrap();
            line.clear();
            read.read_line(&mut line).await.unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap(),
                json!({"id":"approval","result":{"decision":"decline"}})
            );
            write
                .write_all(b"{\"id\":1,\"result\":{\"turn\":{\"id\":\"t\"}}}\n")
                .await
                .unwrap();
        });
        assert_eq!(
            rpc.call("turn/start", json!({})).await.unwrap()["turn"]["id"],
            "t"
        );
        assert_eq!(rpc.next().await.unwrap()["params"]["delta"], "hello");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn rpc_eof_and_server_failure_are_errors() {
        let mut rpc = Rpc::new(
            &b"{\"id\":1,\"error\":{\"message\":\"unsupported\"}}\n"[..],
            tokio::io::sink(),
        );
        assert!(
            rpc.call("initialize", json!({}))
                .await
                .unwrap_err()
                .to_string()
                .contains("unsupported")
        );
        assert!(
            rpc.next()
                .await
                .unwrap_err()
                .to_string()
                .contains("lezárta")
        );
    }

    #[tokio::test]
    async fn interrupt_preserves_racing_completion_and_checks_turn_identity() {
        let (client, server) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let mut rpc = Rpc::new(read, write);
        let server = tokio::spawn(async move {
            let (read, mut write) = tokio::io::split(server);
            let mut read = BufReader::new(read);
            let mut line = String::new();
            read.read_line(&mut line).await.unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&line).unwrap(),
                json!({"id":1,"method":"turn/interrupt","params":{"threadId":"thread-1","turnId":"turn-1"}})
            );
            write.write_all(b"{\"method\":\"turn/completed\",\"params\":{\"threadId\":\"thread-1\",\"turn\":{\"id\":\"turn-1\",\"status\":\"interrupted\"}}}\n{\"id\":1,\"result\":{}}\n").await.unwrap();
        });
        rpc.call(
            "turn/interrupt",
            json!({"threadId":"thread-1","turnId":"turn-1"}),
        )
        .await
        .unwrap();
        let completion = rpc.next().await.unwrap();
        assert_eq!(completion["params"]["turn"]["id"], "turn-1");
        assert!(completed_turn(&completion["params"]["turn"], "secret").is_err());
        server.await.unwrap();
    }

    #[tokio::test]
    #[ignore = "Requires an installed Codex CLI; synthetic token, initialize/thread only, no inference"]
    async fn installed_cli_starts_thread_without_inference() {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let project_path = project.path().canonicalize().unwrap();
        let mut server = Server::start(
            &project_path,
            home.path(),
            "vibe-test-synthetic-token-no-real-account",
        )
        .await
        .unwrap();
        let thread = server
            .open_thread(
                &project_path,
                "gpt-6.1-sol",
                crate::app::Effort::default(),
                None,
            )
            .await
            .unwrap();
        assert!(!thread.is_empty());
        // Untouched threads have no rollout. Refresh before the first turn must start a new one.
        server.stop().await;
        let mut replacement = Server::start(
            &project_path,
            home.path(),
            "vibe-test-refreshed-synthetic-token",
        )
        .await
        .unwrap();
        let restarted = replacement
            .open_thread(
                &project_path,
                "gpt-6.1-sol",
                crate::app::Effort::default(),
                None,
            )
            .await
            .unwrap();
        assert!(!restarted.is_empty());
        assert_ne!(restarted, thread);
        replacement.stop().await;
    }

    #[tokio::test]
    async fn fragmented_frame_survives_cancelled_read_and_rejects_unknown_requests() {
        let (client, mut server) = tokio::io::duplex(4096);
        let (read, write) = tokio::io::split(client);
        let mut rpc = Rpc::new(read, write);
        server.write_all(b"{\"id\":\"s\",").await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(5), rpc.read())
                .await
                .is_err()
        );
        server
            .write_all(b"\"method\":\"dangerous/newMethod\"}\n")
            .await
            .unwrap();
        let request = rpc.read().await.unwrap();
        assert!(rpc.reject_server_request(&request).await.unwrap());
        let mut bytes = [0; 512];
        let length = server.read(&mut bytes).await.unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&bytes[..length]).unwrap()["error"]["code"],
            -32601
        );
    }

    #[test]
    fn only_completed_turn_is_success_and_tokens_are_redacted() {
        assert!(completed_turn(&json!({"status":"completed"}), "secret").is_ok());
        assert!(completed_turn(&json!({"status":"interrupted"}), "secret").is_err());
        assert!(
            completed_turn(
                &json!({"status":"failed","error":{"message":"secret denied"}}),
                "secret"
            )
            .unwrap_err()
            .to_string()
            .contains("[redacted]")
        );
        assert!(completed_turn(&json!({"status":"inProgress"}), "").is_err());
        assert!(
            server_arguments()
                .contains(&"model_providers.openai_chatgpt_plan.requires_openai_auth=false".into())
        );
    }
}
