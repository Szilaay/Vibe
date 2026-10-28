//! Public-client Sign in with ChatGPT. Tokens never enter URLs or diagnostics.
use std::{
    collections::HashMap,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use futures_util::StreamExt;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header, jwk::JwkSet};
use rand::{RngCore, rngs::OsRng};
use serde::{Deserialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::mpsc::UnboundedSender,
};
use url::Url;

use crate::auth_store::{CredentialRecord, Registration, Store, StoreLock};

const ISSUER: &str = "https://auth.openai.com";
const AUTHORIZE: &str = "https://auth.openai.com/api/accounts/authorize";
const TOKEN: &str = "https://auth.openai.com/api/accounts/oauth/token";
const JWKS: &str = "https://auth.openai.com/.well-known/jwks.json";
const REVOKE: &str = "https://auth.openai.com/api/accounts/oauth/revoke";
const RESOURCE: &str = "https://api.openai.com/v1";
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
const INVOKE_SCOPE: &str = "resource.invoke";
const CALLBACK_PATH: &str = "/auth/callback";
const AUTH_TIMEOUT: Duration = Duration::from_secs(300);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_TOKEN_RESPONSE: usize = 48 * 1024;
const MAX_JWKS: usize = 1024 * 1024;
const MAX_CALLBACK: usize = 8192;
static PENDING_REFRESHES: AtomicUsize = AtomicUsize::new(0);
static REFRESH_FINISHED: tokio::sync::Notify = tokio::sync::Notify::const_new();

struct PendingRefresh;

impl PendingRefresh {
    fn new() -> Self {
        PENDING_REFRESHES.fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for PendingRefresh {
    fn drop(&mut self) {
        PENDING_REFRESHES.fetch_sub(1, Ordering::AcqRel);
        REFRESH_FINISHED.notify_waiters();
    }
}

#[derive(Clone, Debug)]
pub struct AccountInfo {
    pub email: Option<String>,
    pub subject: String,
    pub plan_usage: bool,
}

pub enum LoginEvent {
    Waiting { authorization_url: String },
    Completed(AccountInfo),
    Failed(String),
}

pub async fn login(sender: UnboundedSender<LoginEvent>) {
    match login_inner(&sender).await {
        Ok(info) => {
            let _ = sender.send(LoginEvent::Completed(info));
        }
        Err(error) => {
            let _ = sender.send(LoginEvent::Failed(error.to_string()));
        }
    }
}

pub async fn account() -> Result<Option<AccountInfo>> {
    let store = Store::new()?;
    let lock = store.lock().await?;
    let Some(record) = lock.credentials()? else {
        return Ok(None);
    };
    ensure_record_identity(&lock, &record)?;
    Ok(Some(account_info(&record)))
}

/// The renewal owns its lock and task. Dropping an inference request cannot abort a
/// rotating refresh between the server accepting it and the replacement being saved.
pub async fn access_token() -> Result<String> {
    detached_refresh(access_token_inner()).await
}

async fn detached_refresh<F>(renewal: F) -> Result<String>
where
    F: std::future::Future<Output = Result<String>> + Send + 'static,
{
    let pending = PendingRefresh::new();
    tokio::spawn(async move {
        let _pending = pending;
        renewal.await
    })
    .await
    .map_err(|_| anyhow::anyhow!("ChatGPT tokenfrissítés nem fejeződött be."))?
}

/// Call after stopping inference workers and before shutting down Tokio. All HTTP
/// operations and lock acquisition inside a renewal have finite timeouts.
pub async fn finish_pending_refreshes() {
    loop {
        let notified = REFRESH_FINISHED.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if PENDING_REFRESHES.load(Ordering::Acquire) == 0 {
            return;
        }
        notified.await;
    }
}

/// Sign-out completes local clearing even when its UI caller is dropped.
pub async fn logout() -> Result<bool> {
    let pending = PendingRefresh::new();
    tokio::spawn(async move {
        let _pending = pending;
        logout_inner().await
    })
    .await
    .map_err(|_| anyhow::anyhow!("ChatGPT kijelentkezés nem fejeződött be."))?
}

struct Attempt {
    state: String,
    nonce: String,
    verifier: String,
    redirect_uri: String,
    host_id: String,
    registration: Option<Registration>,
}

impl Attempt {
    fn new(port: u16, host_id: String, registration: Option<Registration>) -> Self {
        Self {
            state: random_value(),
            nonce: random_value(),
            verifier: random_value(),
            redirect_uri: format!("http://127.0.0.1:{port}{CALLBACK_PATH}"),
            host_id,
            registration,
        }
    }

    fn authorization_url(&self) -> Result<String> {
        let mut url = Url::parse(AUTHORIZE)?;
        let client_id = self
            .registration
            .as_ref()
            .map_or(DYNAMIC_CLIENT, |r| r.client_id.as_str());
        let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(self.verifier.as_bytes()));
        {
            let mut query = url.query_pairs_mut();
            query
                .append_pair("client_id", client_id)
                .append_pair("response_type", "code")
                .append_pair("redirect_uri", &self.redirect_uri)
                .append_pair("scope", SCOPES)
                .append_pair("resource", RESOURCE)
                .append_pair("state", &self.state)
                .append_pair("nonce", &self.nonce)
                .append_pair("code_challenge_method", "S256")
                .append_pair("code_challenge", &challenge)
                .append_pair("ext_agent_host_id", &self.host_id);
            if self.registration.is_none() {
                query.append_pair("agent_name_hint", "Vibe");
            }
            // Intentionally no id_token_hint: Waiting is user-visible and may be copied.
        }
        Ok(url.into())
    }
}

async fn login_inner(sender: &UnboundedSender<LoginEvent>) -> Result<AccountInfo> {
    let store = Store::new()?;
    let lock = store.lock().await?;
    let host_id = lock.host_id()?;
    let registration = lock.registration()?;
    if let Some(registration) = &registration {
        ensure!(
            valid_client_id(&registration.client_id),
            "Érvénytelen mentett ChatGPT regisztráció."
        );
    }
    drop(lock);
    let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| anyhow::anyhow!("A helyi ChatGPT bejelentkezési port nem nyitható meg."))?;
    let port = listener.local_addr()?.port();
    let attempt = Attempt::new(port, host_id, registration);
    let authorization_url = attempt.authorization_url()?;
    sender
        .send(LoginEvent::Waiting {
            authorization_url: authorization_url.clone(),
        })
        .map_err(|_| anyhow::anyhow!("Bejelentkezés megszakítva."))?;
    // The displayed official URL remains usable when a browser cannot be opened.
    let _ = open::that_detached(authorization_url);
    let callback = tokio::time::timeout(AUTH_TIMEOUT, receive_callback(listener, &attempt, port))
        .await
        .map_err(|_| anyhow::anyhow!("ChatGPT bejelentkezés időtúllépés. Próbáld újra."))??;

    // Do not hold the cross-process lock while the browser waits for the user.
    let lock = store.lock().await?;
    ensure!(
        same_registration(lock.registration()?.as_ref(), attempt.registration.as_ref()),
        "Másik folyamat módosította a ChatGPT fiókot. Indíts új bejelentkezést."
    );
    ensure!(
        lock.host_id()? == attempt.host_id,
        "A helyi ChatGPT hostazonosító megváltozott."
    );
    let client = http_client()?;
    let response = token_request(
        &client,
        &[
            ("grant_type", "authorization_code"),
            ("client_id", &callback.client_id),
            ("code", &callback.code),
            ("code_verifier", &attempt.verifier),
            ("redirect_uri", &attempt.redirect_uri),
            ("resource", RESOURCE),
        ],
    )
    .await
    .map_err(TokenFailure::public_error)?;
    let keys = fetch_jwks(&client).await?;
    let raw_id = response
        .id_token
        .as_deref()
        .context("A ChatGPT válasz nem tartalmazott ID tokent.")?;
    let expected_subject = attempt
        .registration
        .as_ref()
        .map(|registration| registration.subject.as_str());
    let identity = verify_id_token(
        raw_id,
        &keys,
        &callback.client_id,
        NonceCheck::Required(&attempt.nonce),
        expected_subject,
    )?;
    let scopes = response.scopes(None)?;
    let info = AccountInfo {
        email: identity.email.clone(),
        subject: identity.sub.clone(),
        plan_usage: plan_enabled(&scopes),
    };
    let registration = Registration {
        client_id: callback.client_id.clone(),
        subject: identity.sub.clone(),
        email: identity.email.clone(),
    };
    // Even a declined plan permission must retain the verified account/client mapping.
    lock.save_registration(&registration)?;
    if !info.plan_usage {
        lock.clear_credentials()?;
        if !response.has_renewable_tokens() {
            return Ok(info);
        }
    }
    let record =
        response.into_record(callback.client_id, attempt.host_id, identity, scopes, None)?;
    lock.save_credentials(&record)?;
    Ok(info)
}

async fn access_token_inner() -> Result<String> {
    let store = Store::new()?;
    let lock = store.lock().await?;
    let record = lock
        .credentials()?
        .context("Előbb jelentkezz be ChatGPT fiókoddal.")?;
    ensure_record_identity(&lock, &record)?;
    ensure!(
        plan_enabled(&record.scopes),
        "A ChatGPT fiók nem engedélyezte az előfizetés használatát. Jelentkezz be újra."
    );
    if record.expires_at > now()?.saturating_add(120) {
        return Ok(record.access_token);
    }
    let client = http_client()?;
    // Fetch keys before rotating a token, so normal refresh has no further
    // network dependency after the replacement refresh token is received.
    let keys = fetch_jwks(&client).await?;
    let response = match token_request(
        &client,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &record.client_id),
            ("refresh_token", &record.refresh_token),
            ("resource", RESOURCE),
        ],
    )
    .await
    {
        Ok(response) => response,
        Err(failure) => {
            if failure.unusable_refresh() {
                lock.clear_credentials()?;
            }
            return Err(failure.public_error());
        }
    };
    let scopes = response.scopes(Some(&record.scopes))?;
    let identity = if let Some(raw_id) = &response.id_token {
        let nonce = retained_nonce(&record.id_token);
        verify_id_token(
            raw_id,
            &keys,
            &record.client_id,
            NonceCheck::Refresh(nonce.as_deref()),
            Some(&record.subject),
        )?
    } else {
        IdClaims {
            sub: record.subject.clone(),
            email: record.email.clone(),
            nonce: None,
            iat: 0,
            azp: None,
            aud: Audience::One(record.client_id.clone()),
        }
    };
    let new_record = response.into_record(
        record.client_id.clone(),
        record.host_id.clone(),
        identity,
        scopes,
        Some(&record),
    )?;
    // Persist the rotated refresh token even when the refreshed grant lost plan scopes.
    lock.save_credentials(&new_record)?;
    ensure!(
        plan_enabled(&new_record.scopes),
        "A ChatGPT előfizetés használata nincs engedélyezve. Jelentkezz be újra."
    );
    Ok(new_record.access_token)
}

async fn logout_inner() -> Result<bool> {
    let store = Store::new()?;
    let lock = store.lock().await?;
    let credentials = lock.credentials();
    let mut confirmed = false;
    if let Ok(Some(record)) = &credentials
        && let Ok(client) = http_client()
    {
        // Bounded retry only for network/5xx failures. No response body is exposed.
        for attempt in 0..2 {
            let response = client
                .post(REVOKE)
                .form(&[
                    ("token", record.refresh_token.as_str()),
                    ("token_type_hint", "refresh_token"),
                    ("client_id", record.client_id.as_str()),
                ])
                .send()
                .await;
            match response {
                Ok(response) if response.status() == reqwest::StatusCode::OK => {
                    confirmed = bounded_body(response, 1024)
                        .await
                        .is_ok_and(|body| body.is_empty());
                    break;
                }
                Ok(response) if !response.status().is_server_error() => break,
                _ if attempt == 0 => tokio::time::sleep(Duration::from_millis(250)).await,
                _ => break,
            }
        }
    } else if matches!(credentials, Ok(None)) {
        confirmed = true;
    }
    // Clear even corrupt/unreadable credentials; preserve host and registration.
    lock.clear_credentials()?;
    Ok(confirmed)
}

fn account_info(record: &CredentialRecord) -> AccountInfo {
    AccountInfo {
        email: record.email.clone(),
        subject: record.subject.clone(),
        plan_usage: plan_enabled(&record.scopes),
    }
}

fn ensure_record_identity(lock: &StoreLock, record: &CredentialRecord) -> Result<()> {
    let registration = lock
        .registration()?
        .context("Hiányzó ChatGPT regisztráció. Jelentkezz be újra.")?;
    ensure!(
        valid_client_id(&record.client_id)
            && registration.client_id == record.client_id
            && registration.subject == record.subject
            && lock.host_id()? == record.host_id,
        "A mentett ChatGPT fiók és a tokenek nem egyeznek. Jelentkezz be újra."
    );
    Ok(())
}

fn same_registration(left: Option<&Registration>, right: Option<&Registration>) -> bool {
    match (left, right) {
        (None, None) => true,
        (Some(left), Some(right)) => {
            left.client_id == right.client_id && left.subject == right.subject
        }
        _ => false,
    }
}

fn plan_enabled(scopes: &[String]) -> bool {
    scopes.iter().any(|scope| scope == PLAN_SCOPE)
        && scopes.iter().any(|scope| scope == INVOKE_SCOPE)
}

fn random_value() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("Hibás rendszeridő.")?
        .as_secs())
}

fn constant_equal(left: &str, right: &str) -> bool {
    bool::from(left.as_bytes().ct_eq(right.as_bytes()))
}

fn valid_client_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 200
        && value != DYNAMIC_CLIENT
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_-.:".contains(&byte))
}

struct Callback {
    code: String,
    client_id: String,
}

async fn receive_callback(listener: TcpListener, attempt: &Attempt, port: u16) -> Result<Callback> {
    for _ in 0..24 {
        let (mut stream, peer) = listener
            .accept()
            .await
            .map_err(|_| anyhow::anyhow!("A helyi ChatGPT visszahívás nem fogadható."))?;
        if peer.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) {
            continue;
        }
        let request =
            tokio::time::timeout(Duration::from_secs(3), read_callback_request(&mut stream)).await;
        let query = match request {
            Ok(Ok(request)) => parse_callback_request(&request, port),
            _ => Err(anyhow::anyhow!("Érvénytelen helyi visszahívás.")),
        };
        let query = match query {
            Ok(query) => query,
            Err(_) => {
                callback_reply(&mut stream, false).await;
                continue;
            }
        };
        let result = validate_callback(query, attempt);
        callback_reply(&mut stream, result.is_ok()).await;
        return result;
    }
    bail!("Túl sok érvénytelen helyi ChatGPT visszahívás. Indíts új bejelentkezést.")
}

async fn read_callback_request(stream: &mut TcpStream) -> Result<Vec<u8>> {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = stream.read(&mut buffer).await?;
        ensure!(read > 0, "Hiányos visszahívás.");
        request.extend_from_slice(&buffer[..read]);
        ensure!(request.len() <= MAX_CALLBACK, "Túl nagy visszahívás.");
    }
    Ok(request)
}

fn parse_callback_request(request: &[u8], port: u16) -> Result<HashMap<String, String>> {
    ensure!(request.len() <= MAX_CALLBACK, "Túl nagy visszahívás.");
    let request = std::str::from_utf8(request).context("Érvénytelen visszahívás.")?;
    let (head, body) = request
        .split_once("\r\n\r\n")
        .context("Hiányos visszahívás.")?;
    ensure!(body.is_empty(), "A visszahívás nem tartalmazhat törzset.");
    let mut lines = head.split("\r\n");
    let first = lines.next().context("Hiányos visszahívás.")?;
    let parts: Vec<_> = first.split(' ').collect();
    ensure!(
        parts.len() == 3 && parts[0] == "GET" && parts[2] == "HTTP/1.1",
        "Érvénytelen visszahívási metódus."
    );
    let mut host = None;
    for line in lines {
        let (name, value) = line
            .split_once(':')
            .context("Érvénytelen visszahívási fejléc.")?;
        ensure!(
            !name.is_empty() && !name.bytes().any(|b| b.is_ascii_whitespace()),
            "Érvénytelen fejléc."
        );
        if name.eq_ignore_ascii_case("host") {
            ensure!(host.is_none(), "Ismételt host fejléc.");
            host = Some(value.trim());
        }
        ensure!(
            !name.eq_ignore_ascii_case("transfer-encoding"),
            "Érvénytelen visszahívási törzs."
        );
        if name.eq_ignore_ascii_case("content-length") {
            ensure!(value.trim() == "0", "Érvénytelen visszahívási törzs.");
        }
    }
    let expected_host = format!("127.0.0.1:{port}");
    ensure!(
        host == Some(expected_host.as_str()),
        "Érvénytelen visszahívási host."
    );
    let target = parts[1];
    ensure!(
        target.starts_with("/auth/callback?") && !target.contains('#') && target.is_ascii(),
        "Érvénytelen visszahívási útvonal."
    );
    let url = Url::parse(&format!("http://{expected_host}{target}"))?;
    ensure!(
        url.path() == CALLBACK_PATH && url.fragment().is_none(),
        "Érvénytelen visszahívási útvonal."
    );
    let mut query = HashMap::new();
    for (key, value) in url.query_pairs() {
        ensure!(
            !key.is_empty()
                && !value.contains('\u{fffd}')
                && query.insert(key.into_owned(), value.into_owned()).is_none(),
            "Ismételt vagy érvénytelen visszahívási paraméter."
        );
    }
    Ok(query)
}

fn validate_callback(mut query: HashMap<String, String>, attempt: &Attempt) -> Result<Callback> {
    let returned_state = query
        .remove("state")
        .context("Hiányzó bejelentkezési állapot.")?;
    ensure!(
        constant_equal(&returned_state, &attempt.state),
        "A ChatGPT bejelentkezés állapota nem egyezik. Próbáld újra."
    );
    // An OAuth error takes precedence even when an attacker also adds a code.
    if let Some(error) = query.remove("error") {
        if error == "access_denied" {
            bail!("A ChatGPT bejelentkezést vagy jogosultságot elutasítottad.");
        }
        bail!("ChatGPT bejelentkezés sikertelen. Indíts új bejelentkezést.");
    }
    let code = query
        .remove("code")
        .context("A visszahívás nem tartalmazott engedélyezési kódot.")?;
    ensure!(
        !code.is_empty() && code.len() <= 4096 && !code.chars().any(char::is_control),
        "Érvénytelen engedélyezési kód."
    );
    let supplied_client = query.remove("client_id");
    let client_id = match &attempt.registration {
        Some(registration) => {
            if let Some(supplied) = supplied_client {
                ensure!(
                    supplied == registration.client_id,
                    "A ChatGPT regisztráció azonosítója nem egyezik."
                );
            }
            registration.client_id.clone()
        }
        None => supplied_client
            .context("A ChatGPT regisztráció hiányos: nincs kiadott kliensazonosító.")?,
    };
    ensure!(
        valid_client_id(&client_id),
        "Érvénytelen kiadott ChatGPT kliensazonosító."
    );
    Ok(Callback { code, client_id })
}

async fn callback_reply(stream: &mut TcpStream, ok: bool) {
    let body = if ok {
        "<!doctype html><meta charset=utf-8><title>VIBE</title><p>Bejelentkezés feldolgozása. Visszatérhetsz a VIBE alkalmazásba.</p>"
    } else {
        "<!doctype html><meta charset=utf-8><title>VIBE</title><p>A bejelentkezés nem fogadható. Indíts új bejelentkezést a VIBE alkalmazásban.</p>"
    };
    let status = if ok { "200 OK" } else { "400 Bad Request" };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'; frame-ancestors 'none'\r\nX-Content-Type-Options: nosniff\r\n\r\n{body}",
        body.len()
    );
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        stream.write_all(response.as_bytes()),
    )
    .await;
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|_| anyhow::anyhow!("A biztonságos ChatGPT kapcsolat nem indítható."))
}

async fn bounded_body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>> {
    if let Some(length) = response.content_length() {
        ensure!(
            length <= limit as u64,
            "Túl nagy ChatGPT hitelesítési válasz."
        );
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| anyhow::anyhow!("ChatGPT hitelesítési hálózati hiba."))?;
        ensure!(
            body.len().saturating_add(chunk.len()) <= limit,
            "Túl nagy ChatGPT hitelesítési válasz."
        );
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn bounded_json<T: DeserializeOwned>(response: reqwest::Response, limit: usize) -> Result<T> {
    let body = bounded_body(response, limit).await?;
    serde_json::from_slice(&body)
        .map_err(|_| anyhow::anyhow!("Érvénytelen ChatGPT hitelesítési válasz."))
}

async fn fetch_jwks(client: &reqwest::Client) -> Result<JwkSet> {
    let response = client
        .get(JWKS)
        .header(reqwest::header::ACCEPT, "application/json")
        .send()
        .await
        .map_err(|_| anyhow::anyhow!("A ChatGPT aláírókulcsai nem kérhetők le. Próbáld később."))?;
    ensure!(
        response.status() == reqwest::StatusCode::OK,
        "A ChatGPT aláírókulcsai nem érhetők el."
    );
    bounded_json(response, MAX_JWKS).await
}

struct TokenFailure {
    code: Option<String>,
    status: Option<u16>,
}

impl TokenFailure {
    fn unusable_refresh(&self) -> bool {
        matches!(self.status, Some(400 | 401 | 403))
            && matches!(
                self.code.as_deref(),
                Some(
                    "invalid_grant"
                        | "invalid_refresh_token"
                        | "token_expired"
                        | "refresh_token_expired"
                        | "refresh_token_invalidated"
                        | "refresh_token_reused"
                )
            )
    }

    fn public_error(self) -> anyhow::Error {
        if self.unusable_refresh() {
            anyhow::anyhow!("A ChatGPT munkamenet lejárt vagy visszavonták. Jelentkezz be újra.")
        } else if self.code.as_deref() == Some("invalid_client") {
            anyhow::anyhow!(
                "A ChatGPT kliensregisztráció nem fogadható el. Ellenőrizd a bejelentkezési beállítást."
            )
        } else if let Some(status) = self.status {
            anyhow::anyhow!("ChatGPT hitelesítés sikertelen (HTTP {status}). Próbáld később.")
        } else {
            anyhow::anyhow!("ChatGPT hitelesítési hálózati vagy válaszhiba. Próbáld később.")
        }
    }
}

async fn token_request(
    client: &reqwest::Client,
    form: &[(&str, &str)],
) -> std::result::Result<TokenResponse, TokenFailure> {
    let response = client
        .post(TOKEN)
        .header(reqwest::header::ACCEPT, "application/json")
        .form(form)
        .send()
        .await
        .map_err(|_| TokenFailure {
            code: None,
            status: None,
        })?;
    let status = response.status().as_u16();
    if status != 200 {
        #[derive(Deserialize)]
        struct OAuthError {
            error: String,
        }
        let code = bounded_json::<OAuthError>(response, MAX_TOKEN_RESPONSE)
            .await
            .ok()
            .map(|error| error.error);
        return Err(TokenFailure {
            code,
            status: Some(status),
        });
    }
    bounded_json(response, MAX_TOKEN_RESPONSE)
        .await
        .map_err(|_| TokenFailure {
            code: None,
            status: Some(status),
        })
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: Option<String>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
}

impl TokenResponse {
    fn has_renewable_tokens(&self) -> bool {
        self.access_token.as_ref().is_some_and(|s| !s.is_empty())
            && self.refresh_token.as_ref().is_some_and(|s| !s.is_empty())
    }

    fn scopes(&self, previous: Option<&[String]>) -> Result<Vec<String>> {
        if let Some(scope) = &self.scope {
            ensure!(
                scope.len() <= 4096 && !scope.chars().any(|ch| ch.is_control() && ch != ' '),
                "Érvénytelen ChatGPT jogosultságok."
            );
            Ok(scope.split_whitespace().map(str::to_owned).collect())
        } else {
            previous
                .map(<[String]>::to_vec)
                .context("A ChatGPT válasz nem tartalmazott engedélyezett jogosultságokat.")
        }
    }

    fn into_record(
        self,
        client_id: String,
        host_id: String,
        identity: IdClaims,
        scopes: Vec<String>,
        previous: Option<&CredentialRecord>,
    ) -> Result<CredentialRecord> {
        ensure!(
            self.token_type
                .as_deref()
                .is_some_and(|kind| kind.eq_ignore_ascii_case("Bearer")),
            "Nem támogatott ChatGPT tokentípus."
        );
        let expires = self.expires_in.context("Hiányzó ChatGPT tokenlejárat.")?;
        ensure!(
            (1..=86400).contains(&expires),
            "Érvénytelen ChatGPT tokenlejárat."
        );
        let access_token = self
            .access_token
            .context("Hiányzó ChatGPT hozzáférési token.")?;
        // This flow rotates refresh tokens. Never silently reuse the spent token.
        let refresh_token = self
            .refresh_token
            .context("Hiányzó helyettesítő ChatGPT frissítési token. Jelentkezz be újra.")?;
        let id_token = self
            .id_token
            .or_else(|| previous.map(|r| r.id_token.clone()))
            .context("Hiányzó ChatGPT ID token.")?;
        for token in [&access_token, &refresh_token, &id_token] {
            ensure!(
                !token.is_empty()
                    && token.len() <= 16 * 1024
                    && !token.chars().any(char::is_control),
                "Érvénytelen ChatGPT token."
            );
        }
        Ok(CredentialRecord {
            client_id,
            subject: identity.sub,
            email: identity.email,
            host_id,
            id_token,
            access_token,
            refresh_token,
            scopes,
            expires_at: now()?.saturating_add(expires),
        })
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum Audience {
    One(String),
    Many(Vec<String>),
}

#[derive(Deserialize)]
struct IdClaims {
    sub: String,
    email: Option<String>,
    nonce: Option<String>,
    iat: u64,
    azp: Option<String>,
    aud: Audience,
}

enum NonceCheck<'a> {
    Required(&'a str),
    Refresh(Option<&'a str>),
}

fn verify_id_token(
    raw: &str,
    keys: &JwkSet,
    client_id: &str,
    nonce: NonceCheck<'_>,
    expected_subject: Option<&str>,
) -> Result<IdClaims> {
    ensure!(
        raw.len() <= 16 * 1024 && valid_client_id(client_id),
        "Érvénytelen ChatGPT ID token."
    );
    let header =
        decode_header(raw).map_err(|_| anyhow::anyhow!("Érvénytelen ChatGPT ID token fejléc."))?;
    ensure!(
        header.alg == Algorithm::RS256,
        "Nem támogatott ChatGPT ID token aláírás."
    );
    let kid = header
        .kid
        .context("A ChatGPT ID token nem tartalmaz aláírókulcs-azonosítót.")?;
    let matching: Vec<_> = keys
        .keys
        .iter()
        .filter(|key| key.common.key_id.as_deref() == Some(kid.as_str()))
        .collect();
    ensure!(
        matching.len() == 1,
        "Ismeretlen vagy nem egyértelmű ChatGPT aláírókulcs."
    );
    let key = matching[0];
    ensure!(
        matches!(
            key.algorithm,
            jsonwebtoken::jwk::AlgorithmParameters::RSA(_)
        ),
        "Érvénytelen ChatGPT aláírókulcs típusa."
    );
    if let Some(algorithm) = key.common.key_algorithm {
        ensure!(
            algorithm == jsonwebtoken::jwk::KeyAlgorithm::RS256,
            "Érvénytelen ChatGPT aláírókulcs algoritmus."
        );
    }
    if let Some(use_) = &key.common.public_key_use {
        ensure!(
            *use_ == jsonwebtoken::jwk::PublicKeyUse::Signature,
            "Érvénytelen ChatGPT aláírókulcs használat."
        );
    }
    if let Some(operations) = &key.common.key_operations {
        ensure!(
            operations.contains(&jsonwebtoken::jwk::KeyOperations::Verify),
            "A ChatGPT kulcs nem használható aláírás-ellenőrzésre."
        );
    }
    let key = DecodingKey::from_jwk(key)
        .map_err(|_| anyhow::anyhow!("Érvénytelen ChatGPT aláírókulcs."))?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[ISSUER]);
    validation.set_audience(&[client_id]);
    validation.set_required_spec_claims(&["exp", "iat", "iss", "sub", "aud"]);
    validation.leeway = 5;
    validation.validate_nbf = true;
    let identity = decode::<IdClaims>(raw, &key, &validation)
        .map_err(|_| {
            anyhow::anyhow!("A ChatGPT ID token aláírása vagy azonosítói nem ellenőrizhetők.")
        })?
        .claims;
    ensure!(
        !identity.sub.trim().is_empty()
            && identity.sub.len() <= 512
            && !identity.sub.chars().any(char::is_control),
        "Érvénytelen ChatGPT fiókazonosító."
    );
    ensure!(
        identity.iat <= now()?.saturating_add(5),
        "A ChatGPT ID token jövőbeli kiadású. Ellenőrizd a rendszeridőt."
    );
    match nonce {
        NonceCheck::Required(expected) => ensure!(
            identity
                .nonce
                .as_deref()
                .is_some_and(|actual| constant_equal(actual, expected)),
            "A ChatGPT ID token nonce értéke nem egyezik."
        ),
        NonceCheck::Refresh(expected) => {
            if let Some(actual) = &identity.nonce {
                ensure!(
                    expected.is_some_and(|expected| constant_equal(actual, expected)),
                    "A frissített ChatGPT ID token nonce értéke nem egyezik."
                );
            }
        }
    }
    if let Some(subject) = expected_subject {
        ensure!(
            identity.sub == subject,
            "Másik ChatGPT fiók érkezett vissza. A meglévő fiók nem változott."
        );
    }
    if let Audience::Many(audiences) = &identity.aud
        && audiences.len() > 1
    {
        ensure!(
            identity.azp.as_deref() == Some(client_id),
            "A ChatGPT ID token meghatalmazott kliense nem egyezik."
        );
    }
    if let Some(authorized_party) = &identity.azp {
        ensure!(
            authorized_party == client_id,
            "A ChatGPT ID token meghatalmazott kliense nem egyezik."
        );
    }
    if let Audience::One(audience) = &identity.aud {
        ensure!(
            audience == client_id,
            "A ChatGPT ID token kliense nem egyezik."
        );
    }
    if let Some(email) = &identity.email {
        ensure!(
            email.len() <= 320 && !email.chars().any(char::is_control),
            "Érvénytelen ChatGPT fiókadat."
        );
    }
    Ok(identity)
}

// The stored ID token was verified before protected storage. This extracts only
// its original nonce for a refresh response; identity still requires a signature.
fn retained_nonce(raw: &str) -> Option<String> {
    let payload = raw.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    value.get("nonce")?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};
    use serde_json::{Value, json};
    use std::sync::{Arc, atomic::AtomicBool};

    // Public jsonwebtoken 9.3.1 test key, for synthetic signatures only.
    const TEST_PRIVATE_KEY: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDJETqse41HRBsc
7cfcq3ak4oZWFCoZlcic525A3FfO4qW9BMtRO/iXiyCCHn8JhiL9y8j5JdVP2Q9Z
IpfElcFd3/guS9w+5RqQGgCR+H56IVUyHZWtTJbKPcwWXQdNUX0rBFcsBzCRESJL
eelOEdHIjG7LRkx5l/FUvlqsyHDVJEQsHwegZ8b8C0fz0EgT2MMEdn10t6Ur1rXz
jMB/wvCg8vG8lvciXmedyo9xJ8oMOh0wUEgxziVDMMovmC+aJctcHUAYubwoGN8T
yzcvnGqL7JSh36Pwy28iPzXZ2RLhAyJFU39vLaHdljwthUaupldlNyCfa6Ofy4qN
ctlUPlN1AgMBAAECggEAdESTQjQ70O8QIp1ZSkCYXeZjuhj081CK7jhhp/4ChK7J
GlFQZMwiBze7d6K84TwAtfQGZhQ7km25E1kOm+3hIDCoKdVSKch/oL54f/BK6sKl
qlIzQEAenho4DuKCm3I4yAw9gEc0DV70DuMTR0LEpYyXcNJY3KNBOTjN5EYQAR9s
2MeurpgK2MdJlIuZaIbzSGd+diiz2E6vkmcufJLtmYUT/k/ddWvEtz+1DnO6bRHh
xuuDMeJA/lGB/EYloSLtdyCF6sII6C6slJJtgfb0bPy7l8VtL5iDyz46IKyzdyzW
tKAn394dm7MYR1RlUBEfqFUyNK7C+pVMVoTwCC2V4QKBgQD64syfiQ2oeUlLYDm4
CcKSP3RnES02bcTyEDFSuGyyS1jldI4A8GXHJ/lG5EYgiYa1RUivge4lJrlNfjyf
dV230xgKms7+JiXqag1FI+3mqjAgg4mYiNjaao8N8O3/PD59wMPeWYImsWXNyeHS
55rUKiHERtCcvdzKl4u35ZtTqQKBgQDNKnX2bVqOJ4WSqCgHRhOm386ugPHfy+8j
m6cicmUR46ND6ggBB03bCnEG9OtGisxTo/TuYVRu3WP4KjoJs2LD5fwdwJqpgtHl
yVsk45Y1Hfo+7M6lAuR8rzCi6kHHNb0HyBmZjysHWZsn79ZM+sQnLpgaYgQGRbKV
DZWlbw7g7QKBgQCl1u+98UGXAP1jFutwbPsx40IVszP4y5ypCe0gqgon3UiY/G+1
zTLp79GGe/SjI2VpQ7AlW7TI2A0bXXvDSDi3/5Dfya9ULnFXv9yfvH1QwWToySpW
Kvd1gYSoiX84/WCtjZOr0e0HmLIb0vw0hqZA4szJSqoxQgvF22EfIWaIaQKBgQCf
34+OmMYw8fEvSCPxDxVvOwW2i7pvV14hFEDYIeZKW2W1HWBhVMzBfFB5SE8yaCQy
pRfOzj9aKOCm2FjjiErVNpkQoi6jGtLvScnhZAt/lr2TXTrl8OwVkPrIaN0bG/AS
aUYxmBPCpXu3UjhfQiWqFq/mFyzlqlgvuCc9g95HPQKBgAscKP8mLxdKwOgX8yFW
GcZ0izY/30012ajdHY+/QK5lsMoxTnn0skdS+spLxaS5ZEO4qvPVb8RAoCkWMMal
2pOhmquJQVDPDLuZHdrIiKiDM20dy9sMfHygWcZjQ4WSxf/J7T9canLZIXFhHAZT
3wc9h4G8BBCtWN2TN/LsGZdB
-----END PRIVATE KEY-----"#;
    const MODULUS: &str = "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ";
    const CLIENT: &str = "oaiapp_vibe_test";

    fn attempt() -> Attempt {
        let mut value = Attempt::new(
            54321,
            "urn:uuid:00000000-0000-4000-8000-000000000000".into(),
            None,
        );
        value.state = "expected_state".into();
        value.nonce = "expected_nonce".into();
        value
    }

    fn keys() -> JwkSet {
        serde_json::from_value(json!({"keys": [{"kty":"RSA","kid":"test","alg":"RS256","use":"sig","key_ops":["verify"],"n":MODULUS,"e":"AQAB"}]})).unwrap()
    }

    fn claims() -> Value {
        json!({"iss":ISSUER,"sub":"test-subject","aud":CLIENT,"exp":now().unwrap()+300,"iat":now().unwrap(),"nonce":"expected_nonce","email":"fixture@example.invalid"})
    }

    fn signed(claims: &Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some("test".into());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(TEST_PRIVATE_KEY.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    fn verify(claims: &Value) -> Result<IdClaims> {
        verify_id_token(
            &signed(claims),
            &keys(),
            CLIENT,
            NonceCheck::Required("expected_nonce"),
            Some("test-subject"),
        )
    }

    fn http_request(target: &str, host: &str) -> Vec<u8> {
        format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes()
    }

    fn query(target: &str) -> HashMap<String, String> {
        parse_callback_request(&http_request(target, "127.0.0.1:54321"), 54321).unwrap()
    }

    #[test]
    fn authorization_uses_pkce_exact_loopback_and_issued_registration() {
        let mut pending = attempt();
        pending.verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into();
        let url = Url::parse(&pending.authorization_url().unwrap()).unwrap();
        assert_eq!(url.origin().ascii_serialization(), ISSUER);
        let params: HashMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(params["client_id"], DYNAMIC_CLIENT);
        assert_eq!(params["agent_name_hint"], "Vibe");
        assert_eq!(
            params["redirect_uri"],
            "http://127.0.0.1:54321/auth/callback"
        );
        assert_eq!(
            params["code_challenge"],
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert!(!params.contains_key("id_token_hint"));
        pending.registration = Some(Registration {
            client_id: CLIENT.into(),
            subject: "test-subject".into(),
            email: None,
        });
        let url = Url::parse(&pending.authorization_url().unwrap()).unwrap();
        let params: HashMap<_, _> = url
            .query_pairs()
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        assert_eq!(params["client_id"], CLIENT);
        assert!(!params.contains_key("agent_name_hint"));
        assert!(!params.contains_key("id_token_hint"));
        assert_ne!(
            Attempt::new(1, String::new(), None).state,
            Attempt::new(1, String::new(), None).state
        );
    }

    #[test]
    fn callback_requires_exact_host_method_path_and_unique_query() {
        for request in [
            http_request("/auth/callback?state=x", "localhost:54321"),
            http_request("/auth/callback?state=x", "127.0.0.1:1"),
            http_request("/callback?state=x", "127.0.0.1:54321"),
            http_request("/auth/callback?state=x&%73tate=y", "127.0.0.1:54321"),
            http_request("/auth/callback?code=a&code=b", "127.0.0.1:54321"),
            b"POST /auth/callback?state=x HTTP/1.1\r\nHost: 127.0.0.1:54321\r\n\r\n".to_vec(),
            b"GET /auth/callback?state=x HTTP/1.1\r\nHost: 127.0.0.1:54321\r\nHost: 127.0.0.1:54321\r\n\r\n".to_vec(),
            b"GET /auth/callback?state=x HTTP/1.1\r\nHost: 127.0.0.1:54321\r\nContent-Length: 1\r\n\r\nx".to_vec(),
        ] { assert!(parse_callback_request(&request, 54321).is_err()); }
        assert!(parse_callback_request(&vec![b'x'; MAX_CALLBACK + 1], 54321).is_err());
    }

    #[test]
    fn callback_checks_state_error_and_registration_before_code_exchange() {
        let mut pending = attempt();
        let base = "/auth/callback?state=expected_state&code=code";
        assert!(validate_callback(query(base), &pending).is_err());
        assert!(
            validate_callback(
                query(&format!("{base}&client_id=dynamic_agent_client")),
                &pending
            )
            .is_err()
        );
        let callback =
            validate_callback(query(&format!("{base}&client_id={CLIENT}")), &pending).unwrap();
        assert_eq!(callback.client_id, CLIENT);
        assert!(
            validate_callback(
                query(&format!("{base}&client_id={CLIENT}&error=access_denied")),
                &pending
            )
            .is_err()
        );
        assert!(
            validate_callback(
                query(&format!(
                    "/auth/callback?state=wrong&code=x&client_id={CLIENT}"
                )),
                &pending
            )
            .is_err()
        );
        pending.registration = Some(Registration {
            client_id: CLIENT.into(),
            subject: "test-subject".into(),
            email: None,
        });
        assert!(validate_callback(query(base), &pending).is_ok());
        assert!(
            validate_callback(query(&format!("{base}&client_id=oaiapp_other")), &pending).is_err()
        );
    }

    #[test]
    fn signed_identity_rejects_wrong_issuer_audience_expiry_nonce_subject_and_algorithm() {
        assert!(verify(&claims()).is_ok());
        for (field, value) in [
            ("iss", json!("https://attacker.invalid")),
            ("aud", json!("oaiapp_other")),
            ("exp", json!(now().unwrap() - 60)),
            ("nonce", json!("wrong")),
            ("sub", json!("other-account")),
            ("sub", json!("")),
            ("iat", json!(now().unwrap() + 120)),
        ] {
            let mut invalid = claims();
            invalid[field] = value;
            assert!(verify(&invalid).is_err(), "accepted invalid {field}");
        }
        let mut missing = claims();
        missing.as_object_mut().unwrap().remove("nonce");
        assert!(verify(&missing).is_err());
        let mut missing = claims();
        missing.as_object_mut().unwrap().remove("iat");
        assert!(verify(&missing).is_err());
        let mut token = signed(&claims()).into_bytes();
        let position = token.iter().rposition(|b| *b == b'.').unwrap() + 1;
        token[position] = if token[position] == b'A' { b'B' } else { b'A' };
        assert!(
            verify_id_token(
                std::str::from_utf8(&token).unwrap(),
                &keys(),
                CLIENT,
                NonceCheck::Required("expected_nonce"),
                None
            )
            .is_err()
        );
        let weak = encode(
            &Header::new(Algorithm::HS256),
            &claims(),
            &EncodingKey::from_secret(b"test"),
        )
        .unwrap();
        assert!(
            verify_id_token(
                &weak,
                &keys(),
                CLIENT,
                NonceCheck::Required("expected_nonce"),
                None
            )
            .is_err()
        );
        let mut multiple = claims();
        multiple["aud"] = json!([CLIENT, "another"]);
        assert!(verify(&multiple).is_err());
        multiple["azp"] = json!(CLIENT);
        assert!(verify(&multiple).is_ok());
        let mut wrong_party = claims();
        wrong_party["azp"] = json!("oaiapp_other");
        assert!(verify(&wrong_party).is_err());
        let mut duplicate = keys();
        duplicate.keys.push(duplicate.keys[0].clone());
        assert!(
            verify_id_token(
                &signed(&claims()),
                &duplicate,
                CLIENT,
                NonceCheck::Required("expected_nonce"),
                None
            )
            .is_err()
        );
    }

    #[test]
    fn refresh_identity_and_scope_loss_cannot_enable_inference() {
        let raw = signed(&claims());
        assert_eq!(retained_nonce(&raw).as_deref(), Some("expected_nonce"));
        let mut without_nonce = claims();
        without_nonce.as_object_mut().unwrap().remove("nonce");
        assert!(
            verify_id_token(
                &signed(&without_nonce),
                &keys(),
                CLIENT,
                NonceCheck::Refresh(Some("expected_nonce")),
                Some("test-subject")
            )
            .is_ok()
        );
        assert!(
            verify_id_token(
                &raw,
                &keys(),
                CLIENT,
                NonceCheck::Refresh(Some("different")),
                Some("test-subject")
            )
            .is_err()
        );
        assert!(!plan_enabled(&[PLAN_SCOPE.into()]));
        assert!(!plan_enabled(&[INVOKE_SCOPE.into()]));
        assert!(plan_enabled(&[PLAN_SCOPE.into(), INVOKE_SCOPE.into()]));
        for code in [
            "invalid_grant",
            "invalid_refresh_token",
            "token_expired",
            "refresh_token_expired",
            "refresh_token_invalidated",
            "refresh_token_reused",
        ] {
            assert!(
                TokenFailure {
                    code: Some(code.into()),
                    status: Some(400)
                }
                .unusable_refresh()
            );
            assert!(
                !TokenFailure {
                    code: Some(code.into()),
                    status: Some(500)
                }
                .unusable_refresh()
            );
        }
        let error = TokenFailure {
            code: Some("secret-in-response".into()),
            status: Some(400),
        }
        .public_error();
        assert!(!error.to_string().contains("secret"));
    }

    #[test]
    fn token_rotation_requires_replacement_and_keeps_granted_scope_loss() {
        let response = json!({
            "access_token":"new-access", "refresh_token":"new-refresh",
            "id_token":signed(&claims()), "token_type":"Bearer", "expires_in":3600,
            "scope":"openid email"
        });
        let parsed: TokenResponse = serde_json::from_value(response.clone()).unwrap();
        let scopes = parsed.scopes(None).unwrap();
        assert!(!plan_enabled(&scopes));
        let original = parsed
            .into_record(
                CLIENT.into(),
                "urn:uuid:00000000-0000-4000-8000-000000000000".into(),
                verify(&claims()).unwrap(),
                scopes,
                None,
            )
            .unwrap();
        assert_eq!(original.refresh_token, "new-refresh");
        let mut missing = response.clone();
        missing.as_object_mut().unwrap().remove("refresh_token");
        let parsed: TokenResponse = serde_json::from_value(missing).unwrap();
        assert!(
            parsed
                .into_record(
                    CLIENT.into(),
                    original.host_id.clone(),
                    verify(&claims()).unwrap(),
                    original.scopes.clone(),
                    Some(&original)
                )
                .is_err()
        );
        let mut ordinary_refresh = response;
        ordinary_refresh.as_object_mut().unwrap().remove("scope");
        ordinary_refresh.as_object_mut().unwrap().remove("id_token");
        let parsed: TokenResponse = serde_json::from_value(ordinary_refresh).unwrap();
        assert_eq!(
            parsed.scopes(Some(&original.scopes)).unwrap(),
            original.scopes
        );
        let record = parsed
            .into_record(
                CLIENT.into(),
                original.host_id.clone(),
                verify(&claims()).unwrap(),
                original.scopes.clone(),
                Some(&original),
            )
            .unwrap();
        assert_eq!(record.id_token, original.id_token);
        assert!(!account_info(&record).plan_usage);
    }

    async fn local_response(response: &'static [u8]) -> reqwest::Response {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0u8; 1024];
            let _ = stream.read(&mut request).await;
            stream.write_all(response).await.unwrap();
        });
        reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap()
            .get(format!("http://{address}"))
            .send()
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn response_limits_apply_to_declared_and_streamed_bodies() {
        let response = local_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 10000\r\nConnection: close\r\n\r\n",
        )
        .await;
        assert!(bounded_body(response, 8).await.is_err());
        let response = local_response(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n9\r\n123456789\r\n0\r\n\r\n").await;
        assert!(bounded_body(response, 8).await.is_err());
        let response = local_response(
            b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nConnection: close\r\n\r\nsecret",
        )
        .await;
        let error = bounded_json::<TokenResponse>(response, 32)
            .await
            .err()
            .unwrap();
        assert!(!error.to_string().contains("secret"));
    }

    #[tokio::test]
    async fn cancellation_detaches_renewal_and_shutdown_waits_for_persistence() {
        let persisted = Arc::new(AtomicBool::new(false));
        let flag = persisted.clone();
        let (started, waiting) = tokio::sync::oneshot::channel();
        let caller = tokio::spawn(detached_refresh(async move {
            started.send(()).unwrap();
            tokio::time::sleep(Duration::from_millis(30)).await;
            flag.store(true, Ordering::Release);
            Ok("synthetic-access-token".into())
        }));
        waiting.await.unwrap();
        caller.abort();
        let _ = caller.await;
        finish_pending_refreshes().await;
        assert!(persisted.load(Ordering::Acquire));
    }
}
