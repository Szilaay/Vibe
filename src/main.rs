use std::{
    collections::HashMap,
    fs::OpenOptions,
    io::{self, IsTerminal},
    path::PathBuf,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
        MouseEventKind,
    },
    execute,
};
use fs2::FileExt;
use futures_util::StreamExt;
use tokio::{sync::mpsc, task::JoinHandle, time::Instant};
use vibe::{
    app::{App, Message, ModelChoice, PanelStatus, ProviderKind, Role},
    auth::{self, LoginEvent},
    chatgpt,
    provider::{self, ChatRequest, ProviderEvent},
    storage, ui,
};

#[derive(Parser)]
#[command(version, about = "VIBE — több önálló AI-chat egy terminálban")]
struct Cli {
    #[command(subcommand)]
    command: Option<AccountCommand>,
    /// Helyi projekt könyvtára. Fájlok automatikus beküldése nélkül.
    #[arg(short, long, default_value = ".")]
    project: PathBuf,
    /// Helyi mintaválaszok, hálózati kérés és mentés nélkül.
    #[arg(long)]
    demo: bool,
    /// Beszélgetések mentésének és visszatöltésének kikapcsolása.
    #[arg(long)]
    no_save: bool,
    /// Az első panel modellazonosítója a kiválasztott szolgáltatónál.
    #[arg(long)]
    model: Option<String>,
    /// Az első panel kapcsolata: chatgpt, openrouter vagy antigravity (asztali CLI).
    #[arg(long, value_enum)]
    provider: Option<ProviderKind>,
    /// Felület renderelése szövegként, interaktív terminál nélkül.
    #[arg(long)]
    snapshot: bool,
    #[arg(long, default_value_t = 160, value_parser = clap::value_parser!(u16).range(20..=300))]
    width: u16,
    #[arg(long, default_value_t = 48, value_parser = clap::value_parser!(u16).range(10..=120))]
    height: u16,
}

#[derive(Subcommand)]
enum AccountCommand {
    /// Böngészős bejelentkezés ChatGPT-előfizetéssel.
    Login,
    /// Kijelentkezés és helyi tokenek eltávolítása.
    Logout,
    /// Mentett ChatGPT-belépés állapota.
    Account,
}

enum AccountEvent {
    Status(Result<Option<auth::AccountInfo>, String>),
    Models(Result<Vec<chatgpt::ModelInfo>, String>),
    Logout(Result<bool, String>),
}

#[derive(Default)]
struct AccountTasks {
    login: Option<JoinHandle<()>>,
    query: Option<JoinHandle<()>>,
    query_is_logout: bool,
    login_generation: u64,
}

impl Drop for AccountTasks {
    fn drop(&mut self) {
        if let Some(task) = &self.login {
            task.abort();
        }
        // Refresh/revocation must finish storing rotating credentials even if the UI exits.
        // auth::access_token isolates refresh in its own task; the runtime owns completion.
    }
}

async fn account_command(command: AccountCommand) -> Result<()> {
    match command {
        AccountCommand::Login => {
            let (sender, mut receiver) = mpsc::unbounded_channel();
            let task = tokio::spawn(auth::login(sender));
            while let Some(event) = receiver.recv().await {
                match event {
                    LoginEvent::Waiting { authorization_url } => {
                        println!("Continue with ChatGPT — folytasd a böngészőben.");
                        println!("Ha nem nyílt meg: {authorization_url}");
                    }
                    LoginEvent::Completed(info) => {
                        println!(
                            "ChatGPT-fiók: {}",
                            info.email.as_deref().unwrap_or("bejelentkezve")
                        );
                        if !info.plan_usage {
                            bail!(
                                "A fiók nem adott engedélyt az előfizetéses modellhasználatra. Jelentkezz be újra, és engedélyezd a csomag használatát."
                            );
                        }
                        println!("Előfizetéses hozzáférés engedélyezve. Indítás: vibe");
                    }
                    LoginEvent::Failed(message) => bail!("{message}"),
                }
            }
            task.await.context("Belépési folyamat megszakadt")?;
        }
        AccountCommand::Account => match auth::account().await? {
            Some(info) => println!(
                "ChatGPT: {} · előfizetéses engedély: {}",
                info.email.as_deref().unwrap_or("bejelentkezve"),
                if info.plan_usage { "igen" } else { "nem" }
            ),
            None => println!("Nincs ChatGPT-belépés. Bejelentkezés: vibe login"),
        },
        AccountCommand::Logout => {
            let confirmed = auth::logout().await?;
            println!("Helyi ChatGPT-belépés törölve.");
            if !confirmed {
                println!(
                    "Távoli visszavonás nem igazolt; a kapcsolatot a ChatGPT beállításaiban is leválaszthatod."
                );
            }
        }
    }
    Ok(())
}

fn begin_login(
    app: &mut App,
    tasks: &mut AccountTasks,
    sender: &mpsc::UnboundedSender<(u64, LoginEvent)>,
) {
    if app.demo {
        app.notice = "Demóban nincs hálózati belépés. Valódi mód: vibe login".into();
        return;
    }
    if app.auth_busy {
        app.notice = "Fiókművelet folyamatban.".into();
        return;
    }
    app.auth_busy = true;
    app.notice = "ChatGPT-belépés előkészítése…".into();
    tasks.login_generation += 1;
    let generation = tasks.login_generation;
    let output = sender.clone();
    tasks.login = Some(tokio::spawn(async move {
        let (sender, mut receiver) = mpsc::unbounded_channel();
        let login = auth::login(sender);
        tokio::pin!(login);
        loop {
            tokio::select! {
                () = &mut login => {
                    while let Ok(event) = receiver.try_recv() { let _ = output.send((generation, event)); }
                    break;
                },
                Some(event) = receiver.recv() => { let _ = output.send((generation, event)); }
            }
        }
    }));
}

fn begin_models(
    app: &mut App,
    tasks: &mut AccountTasks,
    sender: &mpsc::UnboundedSender<AccountEvent>,
) {
    if app.demo || !app.chatgpt_ready || app.auth_busy {
        return;
    }
    app.auth_busy = true;
    tasks.query_is_logout = false;
    let sender = sender.clone();
    tasks.query = Some(tokio::spawn(async move {
        let result = chatgpt::list_models()
            .await
            .map_err(|error| error.to_string());
        let _ = sender.send(AccountEvent::Models(result));
    }));
}

fn set_account(app: &mut App, info: Option<auth::AccountInfo>) {
    app.chatgpt_ready = info.as_ref().is_some_and(|info| info.plan_usage);
    app.account_label = info
        .map(|info| info.email.unwrap_or_else(|| "ChatGPT-fiók".into()))
        .unwrap_or_else(|| "Nincs bejelentkezve".into());
}

fn account_input(
    app: &mut App,
    workers: &mut Workers,
    tasks: &mut AccountTasks,
    login_sender: &mpsc::UnboundedSender<(u64, LoginEvent)>,
    account_sender: &mpsc::UnboundedSender<AccountEvent>,
    event: KeyEvent,
) -> bool {
    if app.login_url.is_some() {
        if event.code == KeyCode::Esc {
            if let Some(task) = tasks.login.take() {
                task.abort();
            }
            app.login_url = None;
            tasks.login_generation += 1;
            app.auth_busy = false;
            app.notice = "Belépési kísérlet megszakítva.".into();
        }
        return !(event.modifiers.contains(KeyModifiers::CONTROL)
            && event.code == KeyCode::Char('q'));
    }
    if app.model_editor.is_some() || app.show_help {
        return false;
    }
    let command =
        if event.modifiers.contains(KeyModifiers::CONTROL) && event.code == KeyCode::Char('o') {
            Some("/login".to_owned())
        } else if event.code == KeyCode::Enter && event.modifiers.is_empty() {
            Some(app.active_panel().input.trim().to_owned())
        } else {
            None
        };
    let Some(command) = command else {
        return false;
    };
    match command.as_str() {
        "/login" => begin_login(app, tasks, login_sender),
        "/models" => {
            if !app.chatgpt_ready {
                app.notice = "Modellista előtt jelentkezz be: /login".into();
            } else {
                begin_models(app, tasks, account_sender);
                app.model_editor = Some(app.active_panel().model.clone());
            }
        }
        "/logout" => {
            if app.demo {
                app.notice = "Demóban nincs valódi kijelentkezés.".into();
            } else if app.auth_busy {
                app.notice = "Előbb fejezd be a fiókműveletet.".into();
            } else {
                let active = app.active;
                for index in 0..app.panels.len() {
                    if app.panels[index].provider == ProviderKind::Chatgpt {
                        app.active = index;
                        cancel_panel(app, workers);
                    }
                }
                app.active = active;
                app.chatgpt_ready = false;
                app.auth_busy = true;
                let sender = account_sender.clone();
                tasks.query_is_logout = true;
                tasks.query = Some(tokio::spawn(async move {
                    let result = auth::logout().await.map_err(|error| error.to_string());
                    let _ = sender.send(AccountEvent::Logout(result));
                }));
            }
        }
        _ => return false,
    }
    if app.active_panel().input.trim() == command {
        app.active_panel_mut().input.clear();
        app.active_panel_mut().cursor = 0;
    }
    true
}

struct Worker {
    generation: u64,
    task: JoinHandle<()>,
}

#[derive(Default)]
struct Workers {
    running: HashMap<u64, Worker>,
    generation: u64,
    aborted: Vec<JoinHandle<()>>,
}

impl Workers {
    fn stop(&mut self, panel_id: u64) {
        if let Some(worker) = self.running.remove(&panel_id) {
            worker.task.abort();
            self.aborted.push(worker.task);
        }
    }

    async fn shutdown(&mut self) {
        for (_, worker) in self.running.drain() {
            worker.task.abort();
            self.aborted.push(worker.task);
        }
        for task in self.aborted.drain(..) {
            let _ = task.await;
        }
    }

    fn start(
        &mut self,
        request: ChatRequest,
        key: String,
        demo: bool,
        connection: ProviderKind,
        output: mpsc::UnboundedSender<(u64, ProviderEvent)>,
    ) {
        self.stop(request.panel_id);
        self.aborted.retain(|task| !task.is_finished());
        self.generation += 1;
        let generation = self.generation;
        let panel_id = request.panel_id;
        let task = tokio::spawn(async move {
            let (sender, mut receiver) = mpsc::unbounded_channel();
            let inference = async {
                if demo {
                    provider::demo_chat(request, sender).await;
                } else if connection == ProviderKind::Chatgpt {
                    chatgpt::stream_chat(request, sender).await;
                } else if connection == ProviderKind::Antigravity {
                    let _ = sender.send(ProviderEvent::Failed {
                        panel_id: request.panel_id,
                        message:
                            "Antigravity CLI az asztali alkalmazás terminálpaneljében indítható."
                                .into(),
                    });
                } else {
                    provider::stream_chat(request, key, sender).await;
                }
            };
            tokio::pin!(inference);
            loop {
                tokio::select! {
                    () = &mut inference => {
                        while let Ok(event) = receiver.try_recv() { let _ = output.send((generation, event)); }
                        break;
                    },
                    Some(event) = receiver.recv() => { let _ = output.send((generation, event)); }
                }
            }
        });
        self.running.insert(panel_id, Worker { generation, task });
    }
}

impl Drop for Workers {
    fn drop(&mut self) {
        for worker in self.running.values() {
            worker.task.abort();
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        return account_command(command).await;
    }
    let project = cli
        .project
        .canonicalize()
        .context("A projekt könyvtára nem nyitható meg")?;
    if !project.is_dir() {
        bail!("A projekt elérési útja könyvtár legyen.");
    }
    let mut app = App::new(project, cli.demo);
    let key = std::env::var("OPENROUTER_API_KEY").unwrap_or_default();
    // Without an OpenRouter key, new workspaces start with subscription-backed panels.
    if !app.demo && key.trim().is_empty() {
        for index in 0..app.panels.len() {
            app.active = index;
            app.set_provider(ProviderKind::Chatgpt);
        }
        app.active = 0;
    }
    // A preview is read-only and does not lock or load private transcripts.
    let save_path = if cli.demo || cli.no_save || cli.snapshot {
        None
    } else {
        Some(storage::session_path(&app.project_path)?)
    };
    let _lock = if let Some(path) = &save_path {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options.open(path.with_extension("lock"))?;
        file.try_lock_exclusive()
            .context("Ez a projekt már meg van nyitva. Második példány: --no-save")?;
        if let Some(session) = storage::load(path, &app.project_path)? {
            app.panels = session.panels;
            app.workspace_groups = session.workspace_groups;
            app.active_group = session.active_group;
            app.active = session.active;
            app.layout = session.layout;
            app.sidebar_visible = session.sidebar_visible;
            app.next_id = session.next_id;
            // Desktop sessions may be empty; this terminal-only UI needs a composer.
            if app.panels.is_empty() {
                app.add_panel("Chat".into(), "openrouter/auto".into());
            }
            app.notice = "Munkamenet visszatöltve · F1: súgó".into();
        }
        Some(file)
    } else {
        None
    };
    if let Some(provider) = cli.provider {
        app.active = 0;
        app.set_provider(provider);
    }
    if let Some(model) = cli.model {
        app.active = 0;
        if !app.set_model(model) {
            bail!("{}", app.notice);
        }
    }
    if cli.snapshot {
        return snapshot(&app, cli.width, cli.height);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        bail!("Interaktív terminál szükséges. Előnézet: vibe --demo --snapshot");
    }
    if !app.demo && key.trim().is_empty() {
        app.notice = "ChatGPT-előfizetés: Ctrl+O vagy /login · Ctrl+P: panel kapcsolata".into();
    }
    let mut terminal = ratatui::init();
    let _guard = TerminalGuard;
    execute!(io::stdout(), EnableMouseCapture, EnableBracketedPaste)?;
    let result = run(&mut terminal, &mut app, &key, save_path.as_deref()).await;
    auth::finish_pending_refreshes().await;
    result
}

struct TerminalGuard;
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), DisableMouseCapture, DisableBracketedPaste);
        ratatui::restore();
    }
}

fn snapshot(app: &App, width: u16, height: u16) -> Result<()> {
    let backend = ratatui::backend::TestBackend::new(width, height);
    let mut terminal = ratatui::Terminal::new(backend)?;
    terminal.draw(|frame| {
        ui::draw(frame, app);
    })?;
    for y in 0..height {
        let line: String = (0..width)
            .map(|x| terminal.backend().buffer()[(x, y)].symbol())
            .collect();
        println!("{}", line.trim_end());
    }
    Ok(())
}

async fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    key: &str,
    save_path: Option<&std::path::Path>,
) -> Result<()> {
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let mut workers = Workers::default();
    let (login_sender, mut login_receiver) = mpsc::unbounded_channel();
    let (account_sender, mut account_receiver) = mpsc::unbounded_channel();
    let mut accounts = AccountTasks::default();
    if !app.demo {
        let sender = account_sender.clone();
        accounts.query = Some(tokio::spawn(async move {
            let result = auth::account().await.map_err(|error| error.to_string());
            let _ = sender.send(AccountEvent::Status(result));
        }));
    }
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(Duration::from_millis(50));
    let mut dirty = true;
    let mut last_save = Instant::now();
    let mut regions = ui::UiRegions::default();
    let mut redraw = true;
    let outcome: Result<()> = async { loop {
        if redraw {
            terminal.draw(|frame| {
                regions = ui::draw(frame, app);
            })?;
            redraw = false;
        }
        tokio::select! {
            maybe_event = events.next() => {
                let Some(event) = maybe_event else { break; };
                let mut quit = false;
                match event? {
                    Event::Key(event) if event.kind != KeyEventKind::Release => {
                        if !account_input(app, &mut workers, &mut accounts, &login_sender, &account_sender, event) {
                            quit = handle_key(app, &mut workers, key, &sender, event);
                        }
                    },
                    Event::Paste(text) => {
                        if let Some(model) = app.model_editor.as_mut() {
                            model.extend(text.chars().filter(|ch| !ch.is_control() && !ch.is_whitespace()).take(200usize.saturating_sub(model.len())));
                        } else if !app.show_help && app.login_url.is_none() { app.insert(&text); }
                    },
                    Event::Mouse(mouse) if !app.show_help && app.model_editor.is_none() && app.login_url.is_none() => {
                        let point = ratatui::layout::Position::new(mouse.column, mouse.row);
                        if mouse.kind == MouseEventKind::Down(MouseButton::Left) {
                            if regions.login_button.contains(point) { begin_login(app, &mut accounts, &login_sender); }
                            else if regions.new_panel.contains(point) { new_panel(app); }
                            else if regions.layout.contains(point) { app.layout = app.layout.next(); }
                            else if regions.sidebar_toggle.contains(point) { app.sidebar_visible = !app.sidebar_visible; }
                            else if let Some((id, _)) = regions.close_buttons.iter().find(|(_, area)| area.contains(point)) {
                                app.focus_id(*id); close_panel(app, &mut workers);
                            } else if let Some((id, _)) = regions.model_buttons.iter().find(|(_, area)| area.contains(point)) {
                                app.focus_id(*id); open_model_editor(app);
                            } else if let Some((id, _)) = regions.provider_buttons.iter().find(|(_, area)| area.contains(point)) {
                                app.focus_id(*id); toggle_provider(app);
                            } else if let Some((id, _)) = regions.panels.iter().chain(regions.sidebar_panels.iter()).find(|(_, area)| area.contains(point)) { app.focus_id(*id); }
                        } else if matches!(mouse.kind, MouseEventKind::ScrollUp | MouseEventKind::ScrollDown) {
                            if let Some((id, _)) = regions.panels.iter().find(|(_, area)| area.contains(point)) { app.focus_id(*id); }
                            let panel = app.active_panel_mut();
                            panel.scroll = if mouse.kind == MouseEventKind::ScrollUp { panel.scroll.saturating_add(3) } else { panel.scroll.saturating_sub(3) };
                        }
                    },
                    _ => {},
                }
                dirty = true;
                redraw = true;
                if quit { break; }
            },
            Some((generation, event)) = receiver.recv() => {
                apply_event(app, &mut workers, generation, event);
                dirty = true;
                redraw = true;
            },
            Some((generation, event)) = login_receiver.recv() => {
                if generation != accounts.login_generation { continue; }
                match event {
                    LoginEvent::Waiting { authorization_url } => app.login_url = Some(authorization_url),
                    LoginEvent::Completed(info) => {
                        app.login_url = None;
                        app.auth_busy = false;
                        set_account(app, Some(info));
                        if app.chatgpt_ready {
                            if app.active_panel().provider != ProviderKind::Antigravity {
                                app.set_provider(ProviderKind::Chatgpt);
                            }
                            app.notice = "ChatGPT-belépés kész. Előfizetéses modellek betöltése…".into();
                            begin_models(app, &mut accounts, &account_sender);
                        } else { app.notice = "Belépés kész, de az előfizetéses modellhasználat nincs engedélyezve. /login".into(); }
                    },
                    LoginEvent::Failed(message) => {
                        app.login_url = None;
                        app.auth_busy = false;
                        app.notice = message;
                    },
                }
                dirty = true; redraw = true;
            },
            Some(event) = account_receiver.recv() => {
                if matches!(&event, AccountEvent::Status(_)) && accounts.login_generation > 0 { continue; }
                app.auth_busy = false;
                match event {
                    AccountEvent::Status(Ok(info)) => {
                        set_account(app, info);
                        if app.chatgpt_ready { begin_models(app, &mut accounts, &account_sender); }
                    },
                    AccountEvent::Models(Ok(models)) => {
                        app.available_models = models.into_iter().map(|model| ModelChoice { id: model.id, name: model.name }).collect();
                        if let Some(first) = app.available_models.first() {
                            for panel in &mut app.panels {
                                if panel.provider == ProviderKind::Chatgpt && panel.status != PanelStatus::Running && !app.available_models.iter().any(|model| model.id == panel.model) { panel.model = first.id.clone(); }
                            }
                            app.notice = "ChatGPT csatlakoztatva · Ctrl+M: előfizetéses modellválasztó".into();
                        } else { app.notice = "A fiókhoz nem érkezett választható modell. /models: újratöltés".into(); }
                    },
                    AccountEvent::Logout(Ok(revoked)) => {
                        set_account(app, None);
                        app.available_models.clear();
                        app.notice = if revoked { "ChatGPT kijelentkezve.".into() } else { "Helyi belépés törölve; távoli visszavonás nem igazolt. ChatGPT Settings: leválasztás.".into() };
                    },
                    AccountEvent::Status(Err(message)) | AccountEvent::Models(Err(message)) | AccountEvent::Logout(Err(message)) => app.notice = message,
                }
                dirty = true; redraw = true;
            },
            _ = tick.tick() => {
                if dirty && last_save.elapsed() >= Duration::from_secs(2) {
                    if let Some(path) = save_path
                        && let Err(error) = storage::save(path, app) { app.notice = format!("Mentés sikertelen: {error}"); redraw = true; }
                    dirty = false;
                    last_save = Instant::now();
                }
            },
        }
    } Ok(()) }.await;
    if let Some(task) = accounts.login.take() {
        task.abort();
    }
    workers.shutdown().await;
    for panel in &mut app.panels {
        if panel.status == PanelStatus::Running {
            workers.stop(panel.id);
            panel.status = PanelStatus::Cancelled;
            panel.messages.push(Message {
                role: Role::System,
                content: "Válasz megszakítva kilépéskor; részleges szöveg.".into(),
            });
        }
    }
    let save_result = save_path.map(|path| storage::save(path, app)).transpose();
    if let Some(task) = accounts.query.take() {
        if !accounts.query_is_logout {
            task.abort();
        }
        let _ = task.await;
    }
    auth::finish_pending_refreshes().await;
    outcome?;
    save_result.context("Munkamenet mentése sikertelen")?;
    Ok(())
}

fn handle_key(
    app: &mut App,
    workers: &mut Workers,
    key: &str,
    sender: &mpsc::UnboundedSender<(u64, ProviderEvent)>,
    event: KeyEvent,
) -> bool {
    if event.modifiers.contains(KeyModifiers::CONTROL) && event.code == KeyCode::Char('q') {
        return true;
    }
    if app.show_help {
        if matches!(event.code, KeyCode::Esc | KeyCode::F(1)) {
            app.show_help = false;
        }
        return false;
    }
    if let Some(mut model) = app.model_editor.take() {
        match event.code {
            KeyCode::Esc => return false,
            KeyCode::Enter => {
                if !app.set_model(model.clone()) {
                    app.model_editor = Some(model);
                }
                return false;
            }
            KeyCode::Backspace => {
                model.pop();
            }
            KeyCode::Down | KeyCode::Up
                if app.active_panel().provider == ProviderKind::Chatgpt
                    && !app.available_models.is_empty() =>
            {
                let count = app.available_models.len();
                app.model_selection = if event.code == KeyCode::Down {
                    (app.model_selection + 1) % count
                } else {
                    (app.model_selection + count - 1) % count
                };
                model = app.available_models[app.model_selection].id.clone();
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => model.clear(),
            KeyCode::Char(ch)
                if !event
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
                    && !ch.is_whitespace()
                    && !ch.is_control()
                    && model.len() < 200 =>
            {
                model.push(ch)
            }
            _ => {}
        }
        app.model_editor = Some(model);
        return false;
    }
    if event.modifiers.contains(KeyModifiers::CONTROL) {
        match event.code {
            KeyCode::Char('n') => new_panel(app),
            KeyCode::Char('w') => close_panel(app, workers),
            KeyCode::Char('l') => app.layout = app.layout.next(),
            KeyCode::Char('b') => app.sidebar_visible = !app.sidebar_visible,
            KeyCode::Char('z') => app.zoomed = !app.zoomed,
            KeyCode::Char('m') => open_model_editor(app),
            KeyCode::Char('p') => toggle_provider(app),
            KeyCode::Char('c') => cancel_panel(app, workers),
            KeyCode::Char('u') => {
                app.active_panel_mut().input.clear();
                app.active_panel_mut().cursor = 0;
            }
            _ => {}
        }
        return false;
    }
    match event.code {
        KeyCode::F(1) => app.show_help = true,
        KeyCode::Tab => app.focus_next(event.modifiers.contains(KeyModifiers::SHIFT)),
        KeyCode::BackTab => app.focus_next(true),
        KeyCode::Esc => cancel_panel(app, workers),
        KeyCode::Char(ch) if event.modifiers.contains(KeyModifiers::ALT) && ch.is_ascii_digit() => {
            if let Some(index) = ch.to_digit(10).and_then(|digit| digit.checked_sub(1))
                && (index as usize) < app.panels.len()
            {
                app.active = index as usize;
            }
        }
        KeyCode::Enter
            if event
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT) =>
        {
            app.insert("\n")
        }
        KeyCode::Enter => submit(app, workers, key, sender),
        KeyCode::Char(ch) if !event.modifiers.contains(KeyModifiers::ALT) => {
            app.insert(&ch.to_string())
        }
        KeyCode::Backspace => app.backspace(),
        KeyCode::Delete => app.delete(),
        KeyCode::Left => app.move_cursor(false),
        KeyCode::Right => app.move_cursor(true),
        KeyCode::Home => app.active_panel_mut().cursor = 0,
        KeyCode::End => {
            let end = app.active_panel().input.len();
            app.active_panel_mut().cursor = end;
        }
        KeyCode::PageUp => {
            let panel = app.active_panel_mut();
            panel.scroll = panel.scroll.saturating_add(8);
        }
        KeyCode::PageDown => {
            let panel = app.active_panel_mut();
            panel.scroll = panel.scroll.saturating_sub(8);
        }
        _ => {}
    }
    false
}

fn new_panel(app: &mut App) {
    let title = format!("Chat {}", app.next_id);
    let model = app.active_panel().model.clone();
    let provider = app.active_panel().provider;
    if app.add_panel(title, model) {
        app.active_panel_mut().provider = provider;
    }
}

fn open_model_editor(app: &mut App) {
    app.model_selection = app
        .available_models
        .iter()
        .position(|model| model.id == app.active_panel().model)
        .unwrap_or(0);
    app.model_editor = Some(app.active_panel().model.clone());
}

fn toggle_provider(app: &mut App) {
    let next = if app.active_panel().provider == ProviderKind::Chatgpt {
        ProviderKind::OpenRouter
    } else {
        ProviderKind::Chatgpt
    };
    app.set_provider(next);
}

fn close_panel(app: &mut App, workers: &mut Workers) {
    if let Some(id) = app.close_active() {
        workers.stop(id);
    }
}

fn cancel_panel(app: &mut App, workers: &mut Workers) {
    let panel = app.active_panel_mut();
    if panel.status == PanelStatus::Running {
        workers.stop(panel.id);
        panel.status = PanelStatus::Cancelled;
        panel.messages.push(Message {
            role: Role::System,
            content: "Válasz megszakítva; a megmaradt szöveg részleges.".into(),
        });
        app.notice = "Aktív panel leállítva. A többi panel folytatja a munkát.".into();
    }
}

fn submit(
    app: &mut App,
    workers: &mut Workers,
    key: &str,
    sender: &mpsc::UnboundedSender<(u64, ProviderEvent)>,
) {
    if app.active_panel().status == PanelStatus::Running {
        app.notice = "Ez a panel még válaszol. Esc: megszakítás.".into();
        return;
    }
    let text = app.active_panel().input.trim().to_owned();
    if text.is_empty() {
        return;
    }
    if text.starts_with('/') {
        let handled = if let Some(model) = text.strip_prefix("/model ") {
            app.set_model(model.to_owned());
            true
        } else if let Some(provider) = text.strip_prefix("/provider ") {
            match provider.trim() {
                "chatgpt" | "openai" => {
                    app.set_provider(ProviderKind::Chatgpt);
                }
                "openrouter" => {
                    app.set_provider(ProviderKind::OpenRouter);
                }
                _ => app.notice = "Kapcsolat: /provider chatgpt vagy /provider openrouter".into(),
            }
            true
        } else if let Some(title) = text.strip_prefix("/title ") {
            let title: String = title
                .trim()
                .chars()
                .filter(|ch| !ch.is_control())
                .take(60)
                .collect();
            if !title.is_empty() {
                app.active_panel_mut().title = title;
            }
            true
        } else {
            match text.as_str() {
                "/help" => {
                    app.show_help = true;
                    true
                }
                "/clear" => {
                    app.active_panel_mut().messages.clear();
                    app.active_panel_mut().usage = None;
                    true
                }
                "/model" => {
                    open_model_editor(app);
                    true
                }
                _ => false,
            }
        };
        if handled {
            app.active_panel_mut().input.clear();
            app.active_panel_mut().cursor = 0;
            return;
        }
        if text == "/new" {
            app.active_panel_mut().input.clear();
            app.active_panel_mut().cursor = 0;
            new_panel(app);
            return;
        }
    }
    if !app.demo && app.active_panel().provider == ProviderKind::Antigravity {
        app.notice = "Antigravity CLI az asztali alkalmazás terminálpaneljében indítható. A piszkozat megmaradt.".into();
        return;
    }
    if !app.demo && app.active_panel().provider == ProviderKind::Chatgpt && !app.chatgpt_ready {
        app.notice = "ChatGPT-belépés szükséges: Ctrl+O vagy /login. A piszkozat megmaradt.".into();
        return;
    }
    if !app.demo && app.active_panel().provider == ProviderKind::OpenRouter && key.trim().is_empty()
    {
        app.notice =
            "Hiányzó OPENROUTER_API_KEY. A piszkozat megmaradt. Kipróbálás: vibe --demo".into();
        return;
    }
    let demo = app.demo;
    let connection = app.active_panel().provider;
    let panel = app.active_panel_mut();
    panel.messages.push(Message {
        role: Role::User,
        content: text,
    });
    panel.input.clear();
    panel.cursor = 0;
    panel.scroll = 0;
    panel.status = PanelStatus::Running;
    panel.usage = None;
    let request = ChatRequest {
        panel_id: panel.id,
        model: panel.model.clone(),
        messages: panel.messages.clone(),
    };
    panel.messages.push(Message {
        role: Role::Assistant,
        content: String::new(),
    });
    workers.start(request, key.to_owned(), demo, connection, sender.clone());
}

fn apply_event(app: &mut App, workers: &mut Workers, generation: u64, event: ProviderEvent) {
    let id = match &event {
        ProviderEvent::PreparedInput { panel_id, .. } => *panel_id,
        ProviderEvent::Activity { panel_id, .. }
        | ProviderEvent::Review { panel_id, .. }
        | ProviderEvent::Delta { panel_id, .. }
        | ProviderEvent::Usage { panel_id, .. }
        | ProviderEvent::Finished { panel_id }
        | ProviderEvent::Failed { panel_id, .. } => *panel_id,
    };
    if !workers
        .running
        .get(&id)
        .is_some_and(|worker| worker.generation == generation)
    {
        return;
    }
    let Some(panel) = app.panels.iter_mut().find(|panel| panel.id == id) else {
        return;
    };
    match event {
        ProviderEvent::PreparedInput { content, .. } => {
            if let Some(message) = panel.messages.iter_mut().rfind(|m| m.role == Role::User) {
                message.content = content;
            }
            panel.attachments.clear();
        }
        ProviderEvent::Activity { text, .. } => {
            panel.messages.push(Message {
                role: Role::System,
                content: text,
            });
            panel.messages.push(Message {
                role: Role::Assistant,
                content: String::new(),
            });
        }
        ProviderEvent::Review { reply, .. } => {
            let _ = reply.send(false);
        }
        ProviderEvent::Delta { text, .. } => {
            if let Some(message) = panel
                .messages
                .last_mut()
                .filter(|message| message.role == Role::Assistant)
            {
                message.content.extend(
                    text.chars()
                        .filter(|ch| !ch.is_control() || *ch == '\n' || *ch == '\t'),
                );
            }
        }
        ProviderEvent::Usage { usage, .. } => panel.usage = Some(usage),
        ProviderEvent::Finished { .. } => {
            panel.status = PanelStatus::Ready;
            workers.running.remove(&id);
        }
        ProviderEvent::Failed { message, .. } => {
            panel.status = PanelStatus::Error;
            panel.messages.push(Message {
                role: Role::System,
                content: format!("Kérés sikertelen: {message}"),
            });
            workers.running.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn stale_events_after_cancel_cannot_change_panel() {
        let mut app = App::new(PathBuf::from("test"), true);
        let mut workers = Workers::default();
        let (sender, _) = mpsc::unbounded_channel();
        app.insert("teszt");
        submit(&mut app, &mut workers, "", &sender);
        let id = app.active_panel().id;
        let generation = workers.generation;
        cancel_panel(&mut app, &mut workers);
        let count = app.active_panel().messages.len();
        apply_event(
            &mut app,
            &mut workers,
            generation,
            ProviderEvent::Delta {
                panel_id: id,
                text: "stale".into(),
            },
        );
        assert_eq!(app.active_panel().messages.len(), count);
        assert_eq!(app.active_panel().status, PanelStatus::Cancelled);
        assert!(
            !app.active_panel()
                .messages
                .iter()
                .any(|message| message.content == "stale")
        );
    }

    #[test]
    fn missing_key_keeps_draft_and_never_starts_request() {
        let mut app = App::new(PathBuf::from("test"), false);
        let mut workers = Workers::default();
        let (sender, _) = mpsc::unbounded_channel();
        app.insert("őrizd meg");
        submit(&mut app, &mut workers, "", &sender);
        assert_eq!(app.active_panel().input, "őrizd meg");
        assert!(workers.running.is_empty());
        assert_eq!(app.active_panel().status, PanelStatus::Ready);
    }

    #[test]
    fn subscription_panel_requires_login_but_not_an_openrouter_key() {
        let mut app = App::new(PathBuf::from("test"), false);
        app.set_provider(ProviderKind::Chatgpt);
        app.insert("előfizetéses kérdés");
        let mut workers = Workers::default();
        let (sender, _) = mpsc::unbounded_channel();
        submit(&mut app, &mut workers, "", &sender);
        assert_eq!(app.active_panel().input, "előfizetéses kérdés");
        assert!(app.notice.contains("ChatGPT-belépés"));
        assert!(workers.running.is_empty());
        new_panel(&mut app);
        assert_eq!(app.active_panel().provider, ProviderKind::Chatgpt);
    }

    #[tokio::test]
    async fn simultaneous_panels_keep_history_and_cancellation_independent() {
        let mut app = App::new(PathBuf::from("test"), true);
        let mut workers = Workers::default();
        let (sender, mut receiver) = mpsc::unbounded_channel();
        app.insert("csak az első panel kérdése");
        submit(&mut app, &mut workers, "", &sender);
        let first_id = app.active_panel().id;
        app.focus_next(false);
        app.insert("csak a második panel kérdése");
        submit(&mut app, &mut workers, "", &sender);
        let second_id = app.active_panel().id;
        assert_eq!(workers.running.len(), 2);
        app.active = 0;
        cancel_panel(&mut app, &mut workers);
        assert_eq!(app.panels[0].status, PanelStatus::Cancelled);
        assert_eq!(app.panels[1].status, PanelStatus::Running);
        tokio::time::timeout(Duration::from_secs(10), async {
            while workers.running.contains_key(&second_id) {
                let (generation, event) = receiver.recv().await.unwrap();
                apply_event(&mut app, &mut workers, generation, event);
            }
        })
        .await
        .expect("second panel should complete independently");
        assert!(!workers.running.contains_key(&first_id));
        assert_eq!(app.panels[0].status, PanelStatus::Cancelled);
        assert_eq!(app.panels[1].status, PanelStatus::Ready);
        assert!(app.panels[1].messages.iter().any(
            |message| message.role == Role::Assistant && message.content.contains("szimulált")
        ));
        assert!(
            !app.panels[0]
                .messages
                .iter()
                .any(|message| message.content.contains("második"))
        );
        assert!(
            !app.panels[1]
                .messages
                .iter()
                .any(|message| message.content.contains("első"))
        );
    }
}
