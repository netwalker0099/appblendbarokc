//! "Connect Square": holding a Square token through OAuth instead of a personal
//! access token.
//!
//! A personal access token carries every permission on the Square account,
//! including bank accounts and payouts. OAuth lets the owner approve exactly the
//! permissions this app uses ([`SCOPES`]) on Square's own page, and gives the app
//! a token limited to those.
//!
//! ## The flow
//!
//! 1. An admin presses Connect Square. [`Connection::begin`] issues a one-time
//!    `state` bound to that admin and returns Square's authorize URL.
//! 2. The owner signs in to Square and approves. Square sends the browser to
//!    `/api/square/oauth/callback` with a `code` and the `state`.
//! 3. The callback checks the `state` (same admin, unused, under ten minutes
//!    old) and [`Connection::complete`] trades the code for a token.
//!
//! ## Where the token lives
//!
//! In a 0600 file on the secrets volume — the same place, for the same reason,
//! as the Google key: `GET /api/admin/backup` hands an admin a full pg_dump, and a
//! credential in a table would ride along in every copy of it. One file per
//! Square environment, so a sandbox token can never be sent to production.
//!
//! ## Keeping it alive
//!
//! Square access tokens expire after 30 days. The refresh token (code flow, so it
//! does not expire) gets a new one; [`run_refresh_worker`] does that about a week
//! after each issue, which is what Square recommends, and leaves three weeks to
//! notice and fix a refresh that keeps failing before anything stops working.
//!
//! ## Precedence
//!
//! When OAuth credentials for the server's environment are in `.env`, OAuth is
//! the only token source: `SQUARE_ACCESS_TOKEN` is ignored, so the all-access
//! token cannot quietly stay in use after the switch.

use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

use super::{SquareEnv, TokenCell};

/// Exactly the permissions this app uses — see README → "Square permissions".
/// The owner sees this list on Square's approval page.
pub const SCOPES: &[&str] = &[
    "ORDERS_WRITE",
    "ORDERS_READ",
    "PAYMENTS_WRITE",
    "PAYMENTS_READ",
    "DEVICE_CREDENTIAL_MANAGEMENT",
    "CUSTOMERS_READ",
    "CUSTOMERS_WRITE",
];

/// Where tokens are written. A named volume in compose, shared with the Google
/// key, so it survives the container being recreated on deploy.
const TOKEN_DIR: &str = "/var/lib/blendbar/secrets";

/// Used when `SQUARE_OAUTH_REDIRECT_URL` is unset. Must match the Redirect URL
/// on the application's OAuth page in the Square Developer Dashboard exactly.
const DEFAULT_REDIRECT_URL: &str = "https://app.theblendbarokc.com/api/square/oauth/callback";

/// How long a Connect Square attempt may take, from pressing the button to
/// arriving back from Square.
const STATE_TTL: Duration = Duration::from_secs(10 * 60);

/// Refresh once fewer than this many days remain. Tokens last 30 days, so this
/// renews a week after each issue.
const REFRESH_WHEN_DAYS_LEFT: i64 = 23;

/// How often the worker checks whether a refresh is due.
const REFRESH_CHECK_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// The Square application's OAuth credentials, read from `.env`.
#[derive(Clone)]
pub struct OAuthConfig {
    pub app_id: String,
    secret: String,
    pub env: SquareEnv,
    pub redirect_url: String,
}

/// Whether Connect Square can be used on this server.
pub enum OAuthSetup {
    /// No application id and secret in `.env`.
    NotConfigured,
    /// The credentials are for the other Square environment. Production
    /// application ids start `sq0idp-`, sandbox ones `sandbox-sq0idb-`.
    WrongEnvironment { app_env: SquareEnv },
    Ready(OAuthConfig),
}

impl OAuthSetup {
    pub fn from_env(env: SquareEnv) -> Self {
        let var = |key: &str| {
            std::env::var(key)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        };
        let (Some(app_id), Some(secret)) = (
            var("SQUARE_OAUTH_APPLICATION_ID"),
            var("SQUARE_OAUTH_APPLICATION_SECRET"),
        ) else {
            return OAuthSetup::NotConfigured;
        };
        let app_env = app_env_of(&app_id);
        if app_env != env {
            return OAuthSetup::WrongEnvironment { app_env };
        }
        OAuthSetup::Ready(OAuthConfig {
            app_id,
            secret,
            env,
            redirect_url: var("SQUARE_OAUTH_REDIRECT_URL")
                .unwrap_or_else(|| DEFAULT_REDIRECT_URL.to_string()),
        })
    }

    pub fn ready(&self) -> Option<&OAuthConfig> {
        match self {
            OAuthSetup::Ready(cfg) => Some(cfg),
            _ => None,
        }
    }
}

fn app_env_of(app_id: &str) -> SquareEnv {
    if app_id.starts_with("sandbox-") {
        SquareEnv::Sandbox
    } else {
        SquareEnv::Production
    }
}

/// Square's approval page for this app and these permissions.
fn authorize_url(cfg: &OAuthConfig, state: &str) -> String {
    let base = format!("{}/oauth2/authorize", cfg.env.base_url());
    let scope = SCOPES.join(" ");
    let mut params = vec![
        ("client_id", cfg.app_id.as_str()),
        ("scope", scope.as_str()),
        ("state", state),
        ("redirect_uri", cfg.redirect_url.as_str()),
    ];
    // Production only: make Square ask who is signing in rather than reuse
    // whichever Square account the browser happens to be logged in to.
    if cfg.env == SquareEnv::Production {
        params.push(("session", "false"));
    }
    Url::parse_with_params(&base, &params)
        .expect("Square authorize URL is well-formed")
        .to_string()
}

/// A token as held on disk. Never sent to a browser.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredToken {
    pub env: String,
    pub merchant_id: Option<String>,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: DateTime<Utc>,
    /// As reported by Square for this token, not as requested.
    pub scopes: Vec<String>,
    pub connected_at: DateTime<Utc>,
    pub refreshed_at: DateTime<Utc>,
}

fn token_path(dir: &Path, env: SquareEnv) -> PathBuf {
    dir.join(format!("square-oauth-{}.json", env.label()))
}

/// Write 0600 to a temporary file, then move it into place, so there is never a
/// moment when the token is readable by anything else or half-written.
fn store_in(dir: &Path, env: SquareEnv, token: &StoredToken) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let path = token_path(dir, env);
    let tmp = path.with_extension("tmp");

    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let body = serde_json::to_vec(token).map_err(|e| format!("could not encode the token: {e}"))?;
    let mut file = options
        .open(&tmp)
        .map_err(|e| format!("could not write the token: {e}"))?;
    file.write_all(&body)
        .and_then(|_| file.sync_all())
        .map_err(|e| format!("could not write the token: {e}"))?;
    drop(file);
    std::fs::rename(&tmp, &path).map_err(|e| format!("could not save the token: {e}"))
}

fn load_from(dir: &Path, env: SquareEnv) -> Result<Option<StoredToken>, String> {
    let path = token_path(dir, env);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    let token: StoredToken = serde_json::from_slice(&raw)
        .map_err(|e| format!("{} is not a valid token file: {e}", path.display()))?;
    if token.env != env.label() {
        return Err(format!(
            "{} holds a {} token, not {}",
            path.display(),
            token.env,
            env.label()
        ));
    }
    Ok(Some(token))
}

fn remove_from(dir: &Path, env: SquareEnv) -> Result<(), String> {
    let path = token_path(dir, env);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not remove {}: {e}", path.display())),
    }
}

fn refresh_due(expires_at: DateTime<Utc>, now: DateTime<Utc>) -> bool {
    expires_at - now < chrono::Duration::days(REFRESH_WHEN_DAYS_LEFT)
}

/// Permissions we ask for that the token does not have, and ones it has that we
/// never asked for. Either is worth showing: the first breaks something, the
/// second is more access than this app should hold.
fn scope_gaps(granted: &[String]) -> (Vec<String>, Vec<String>) {
    let missing = SCOPES
        .iter()
        .filter(|s| !granted.iter().any(|g| g == *s))
        .map(|s| s.to_string())
        .collect();
    let extra = granted
        .iter()
        .filter(|g| !SCOPES.contains(&g.as_str()))
        .cloned()
        .collect();
    (missing, extra)
}

/// One-time `state` values for Connect Square attempts in progress. Held in
/// memory: an attempt is a few minutes long, and one cut short by a restart
/// just needs the button pressing again.
#[derive(Default)]
struct PendingStates {
    inner: Mutex<HashMap<String, (Uuid, Instant)>>,
}

impl PendingStates {
    fn issue(&self, employee_id: Uuid) -> String {
        let state = crate::auth::generate_token();
        let mut map = self.inner.lock().expect("oauth state lock poisoned");
        map.retain(|_, (_, at)| at.elapsed() < STATE_TTL);
        map.insert(state.clone(), (employee_id, Instant::now()));
        state
    }

    /// Single use: a state is gone after the first look, whether it matched or not.
    fn take(&self, state: &str, employee_id: Uuid) -> bool {
        let mut map = self.inner.lock().expect("oauth state lock poisoned");
        match map.remove(state) {
            Some((owner, at)) => owner == employee_id && at.elapsed() < STATE_TTL,
            None => false,
        }
    }
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_at: DateTime<Utc>,
    merchant_id: Option<String>,
    refresh_token: Option<String>,
}

/// The app's link to a Square account, and the token the HTTP client sends.
pub struct Connection {
    pub setup: OAuthSetup,
    /// What the live client reads its bearer token from. `None` when the app
    /// is on the mock, in which case there is nothing to connect.
    cell: Option<TokenCell>,
    /// True when the token in `cell` came from `SQUARE_ACCESS_TOKEN`.
    personal_token: bool,
    stored: RwLock<Option<StoredToken>>,
    last_error: Mutex<Option<String>>,
    states: PendingStates,
    /// Held across a refresh so the worker and "Renew now" can't both spend
    /// the same refresh token.
    refresh_lock: tokio::sync::Mutex<()>,
    dir: PathBuf,
    http: Client,
}

impl Connection {
    pub fn new(setup: OAuthSetup, cell: Option<TokenCell>, personal_token: bool) -> Self {
        Self {
            setup,
            cell,
            personal_token,
            stored: RwLock::new(None),
            last_error: Mutex::new(None),
            states: PendingStates::default(),
            refresh_lock: tokio::sync::Mutex::new(()),
            dir: PathBuf::from(TOKEN_DIR),
            http: Client::builder()
                .user_agent("blendbar-app/0.2")
                .timeout(Duration::from_secs(20))
                .build()
                .expect("failed to build reqwest client"),
        }
    }

    fn set_error(&self, err: Option<String>) {
        *self.last_error.lock().expect("oauth error lock poisoned") = err;
    }

    fn install(&self, token: Option<StoredToken>) {
        if let Some(cell) = &self.cell {
            *cell.write().expect("square token lock poisoned") =
                token.as_ref().map(|t| Arc::from(t.access_token.as_str()));
        }
        *self.stored.write().expect("oauth token lock poisoned") = token;
    }

    /// Pick up a token saved before the last restart.
    pub fn load_stored(&self) {
        let Some(cfg) = self.setup.ready() else { return };
        match load_from(&self.dir, cfg.env) {
            Ok(Some(token)) => {
                tracing::info!(
                    env = cfg.env.label(),
                    merchant_id = token.merchant_id.as_deref().unwrap_or("?"),
                    expires_at = %token.expires_at,
                    "square: connected through OAuth"
                );
                self.install(Some(token));
            }
            Ok(None) => tracing::warn!(
                env = cfg.env.label(),
                "square: OAuth is configured but not connected yet — an admin needs to \
                 press Connect Square in Admin → Billing. No payments until then."
            ),
            Err(e) => {
                tracing::error!("square: stored OAuth token unusable: {e}");
                self.set_error(Some(e));
            }
        }
    }

    /// Start a Connect Square attempt; returns the URL to send the browser to.
    pub fn begin(&self, employee_id: Uuid) -> Result<String, String> {
        let cfg = self.ready_config()?;
        let state = self.states.issue(employee_id);
        Ok(authorize_url(cfg, &state))
    }

    fn ready_config(&self) -> Result<&OAuthConfig, String> {
        if self.cell.is_none() {
            return Err("SQUARE_LOCATION_ID is not set, so the app is on its mock and \
                        there is nothing to connect yet"
                .into());
        }
        match &self.setup {
            OAuthSetup::Ready(cfg) => Ok(cfg),
            OAuthSetup::NotConfigured => Err("SQUARE_OAUTH_APPLICATION_ID and \
                 SQUARE_OAUTH_APPLICATION_SECRET are not set in .env"
                .into()),
            OAuthSetup::WrongEnvironment { app_env } => Err(format!(
                "the Square application credentials in .env are for {}, but the server \
                 is running against {}",
                app_env.label(),
                if *app_env == SquareEnv::Production { "sandbox" } else { "production" }
            )),
        }
    }

    /// Finish an attempt: check the state, trade the code for a token, save it.
    pub async fn complete(&self, employee_id: Uuid, state: &str, code: &str) -> Result<(), String> {
        let cfg = self.ready_config()?.clone();
        if !self.states.take(state, employee_id) {
            return Err("this sign-in link has expired or was already used — press \
                        Connect Square again"
                .into());
        }

        let _guard = self.refresh_lock.lock().await;
        let resp = self
            .token_request(
                &cfg,
                json!({
                    "client_id": cfg.app_id,
                    "client_secret": cfg.secret,
                    "grant_type": "authorization_code",
                    "code": code,
                    "redirect_uri": cfg.redirect_url,
                }),
            )
            .await?;
        let refresh_token = resp
            .refresh_token
            .clone()
            .ok_or("Square did not return a refresh token")?;
        let scopes = self.granted_scopes(&cfg, &resp.access_token).await;

        let now = Utc::now();
        let token = StoredToken {
            env: cfg.env.label().to_string(),
            merchant_id: resp.merchant_id,
            access_token: resp.access_token,
            refresh_token,
            expires_at: resp.expires_at,
            scopes,
            connected_at: now,
            refreshed_at: now,
        };
        store_in(&self.dir, cfg.env, &token)?;
        tracing::info!(
            env = cfg.env.label(),
            merchant_id = token.merchant_id.as_deref().unwrap_or("?"),
            "square: connected through OAuth"
        );
        self.install(Some(token));
        self.set_error(None);
        Ok(())
    }

    /// Get a new access token if the current one is due (or `force`).
    pub async fn refresh(&self, force: bool) -> Result<bool, String> {
        let Some(cfg) = self.setup.ready().cloned() else { return Ok(false) };
        let _guard = self.refresh_lock.lock().await;
        let Some(current) = self.stored.read().expect("oauth token lock poisoned").clone() else {
            return Ok(false);
        };
        if !force && !refresh_due(current.expires_at, Utc::now()) {
            return Ok(false);
        }

        let result = self
            .token_request(
                &cfg,
                json!({
                    "client_id": cfg.app_id,
                    "client_secret": cfg.secret,
                    "grant_type": "refresh_token",
                    "refresh_token": current.refresh_token,
                }),
            )
            .await;
        let resp = match result {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("renewing the Square token failed: {e}");
                tracing::error!(expires_at = %current.expires_at, "square: {msg}");
                self.set_error(Some(msg.clone()));
                return Err(msg);
            }
        };
        let scopes = self.granted_scopes(&cfg, &resp.access_token).await;
        let token = StoredToken {
            access_token: resp.access_token,
            // Code-flow refresh tokens don't rotate, but take a new one if offered.
            refresh_token: resp.refresh_token.unwrap_or(current.refresh_token),
            expires_at: resp.expires_at,
            merchant_id: resp.merchant_id.or(current.merchant_id),
            scopes: if scopes.is_empty() { current.scopes } else { scopes },
            refreshed_at: Utc::now(),
            ..current
        };
        store_in(&self.dir, cfg.env, &token)?;
        tracing::info!(expires_at = %token.expires_at, "square: OAuth token renewed");
        self.install(Some(token));
        self.set_error(None);
        Ok(true)
    }

    /// Revoke the token at Square (best effort) and forget it here.
    pub async fn disconnect(&self) -> Result<(), String> {
        let cfg = self.ready_config()?.clone();
        let _guard = self.refresh_lock.lock().await;
        let current = self.stored.read().expect("oauth token lock poisoned").clone();
        if let Some(token) = &current {
            let resp = self
                .http
                .post(format!("{}/oauth2/revoke", cfg.env.base_url()))
                .header("Authorization", format!("Client {}", cfg.secret))
                .header("Square-Version", super::http::SQUARE_VERSION)
                .json(&json!({ "client_id": cfg.app_id, "access_token": token.access_token }))
                .send()
                .await;
            match resp {
                Ok(r) if r.status().is_success() => {
                    tracing::info!("square: OAuth token revoked")
                }
                Ok(r) => tracing::warn!(status = r.status().as_u16(), "square: revoke refused"),
                Err(e) => tracing::warn!("square: revoke failed: {e}"),
            }
        }
        remove_from(&self.dir, cfg.env)?;
        self.install(None);
        self.set_error(None);
        Ok(())
    }

    async fn token_request(&self, cfg: &OAuthConfig, body: Value) -> Result<TokenResponse, String> {
        let resp = self
            .http
            .post(format!("{}/oauth2/token", cfg.env.base_url()))
            .header("Square-Version", super::http::SQUARE_VERSION)
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("could not reach Square: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "Square answered {}: {}",
                status.as_u16(),
                super::http::summarize_errors(&text)
            ));
        }
        serde_json::from_str(&text).map_err(|e| format!("unexpected reply from Square: {e}"))
    }

    /// The permissions Square actually granted. Empty if Square won't say;
    /// that only costs the status panel its permissions list.
    async fn granted_scopes(&self, cfg: &OAuthConfig, access_token: &str) -> Vec<String> {
        let resp = self
            .http
            .post(format!("{}/oauth2/token/status", cfg.env.base_url()))
            .bearer_auth(access_token)
            .header("Square-Version", super::http::SQUARE_VERSION)
            .json(&json!({}))
            .send()
            .await;
        let Ok(resp) = resp else { return Vec::new() };
        if !resp.status().is_success() {
            return Vec::new();
        }
        resp.json::<Value>()
            .await
            .ok()
            .and_then(|v| {
                v.get("scopes").and_then(Value::as_array).map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(str::to_string))
                        .collect()
                })
            })
            .unwrap_or_default()
    }

    /// For the admin panel. Never includes a token.
    pub fn status(&self) -> Value {
        let has_token = self
            .cell
            .as_ref()
            .is_some_and(|c| c.read().expect("square token lock poisoned").is_some());
        let method = match (&self.setup, has_token) {
            (OAuthSetup::Ready(_), true) => "oauth",
            (OAuthSetup::Ready(_), false) => "none",
            (_, true) if self.personal_token => "personal_token",
            _ => "none",
        };
        let stored = self.stored.read().expect("oauth token lock poisoned").clone();
        let (missing, extra) = stored
            .as_ref()
            .filter(|t| !t.scopes.is_empty())
            .map(|t| scope_gaps(&t.scopes))
            .unwrap_or_default();

        let (oauth_state, app_env) = match &self.setup {
            OAuthSetup::NotConfigured => ("not_configured", None),
            OAuthSetup::WrongEnvironment { app_env } => ("wrong_environment", Some(*app_env)),
            OAuthSetup::Ready(cfg) => ("ready", Some(cfg.env)),
        };

        json!({
            "method": method,
            "oauth": oauth_state,
            "app_environment": app_env.map(|e| e.label()),
            "can_connect": self.ready_config().is_ok(),
            "redirect_url": self.setup.ready().map(|c| c.redirect_url.clone()),
            "requested_scopes": SCOPES,
            "connected": stored.is_some(),
            "merchant_id": stored.as_ref().and_then(|t| t.merchant_id.clone()),
            "scopes": stored.as_ref().map(|t| t.scopes.clone()),
            "missing_scopes": missing,
            "extra_scopes": extra,
            "connected_at": stored.as_ref().map(|t| t.connected_at),
            "refreshed_at": stored.as_ref().map(|t| t.refreshed_at),
            "expires_at": stored.as_ref().map(|t| t.expires_at),
            "last_error": self.last_error.lock().expect("oauth error lock poisoned").clone(),
        })
    }
}

/// Renew the OAuth token about a week after each issue, for the life of the
/// process. Does nothing unless OAuth is configured and connected.
pub async fn run_refresh_worker(connection: Arc<Connection>) {
    if connection.setup.ready().is_none() {
        return;
    }
    // A short first wait so a token that fell due while the app was down is
    // renewed soon after a restart, not an hour later.
    tokio::time::sleep(Duration::from_secs(60)).await;
    loop {
        // Errors are logged and shown in Admin → Billing by `refresh` itself.
        let _ = connection.refresh(false).await;
        tokio::time::sleep(REFRESH_CHECK_INTERVAL).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(env: SquareEnv) -> OAuthConfig {
        OAuthConfig {
            app_id: "sq0idp-abc".into(),
            secret: "sq0csp-secret".into(),
            env,
            redirect_url: "https://app.example.com/api/square/oauth/callback".into(),
        }
    }

    fn token(env: &str) -> StoredToken {
        let now = Utc::now();
        StoredToken {
            env: env.into(),
            merchant_id: Some("M1".into()),
            access_token: "EAAA-access".into(),
            refresh_token: "EQAA-refresh".into(),
            expires_at: now + chrono::Duration::days(30),
            scopes: vec!["ORDERS_READ".into()],
            connected_at: now,
            refreshed_at: now,
        }
    }

    #[test]
    fn authorize_url_asks_for_exactly_our_scopes() {
        let url = Url::parse(&authorize_url(&cfg(SquareEnv::Production), "st8")).unwrap();
        assert_eq!(url.host_str(), Some("connect.squareup.com"));
        assert_eq!(url.path(), "/oauth2/authorize");
        let q: HashMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["client_id"], "sq0idp-abc");
        assert_eq!(q["state"], "st8");
        assert_eq!(q["session"], "false");
        assert_eq!(q["redirect_uri"], "https://app.example.com/api/square/oauth/callback");
        let scopes: Vec<&str> = q["scope"].split(' ').collect();
        assert_eq!(scopes, SCOPES);
        // The secret never goes to the browser.
        assert!(!url.as_str().contains("secret"));
    }

    #[test]
    fn sandbox_authorize_url_uses_the_sandbox_host_without_session() {
        let url = Url::parse(&authorize_url(&cfg(SquareEnv::Sandbox), "s")).unwrap();
        assert_eq!(url.host_str(), Some("connect.squareupsandbox.com"));
        assert!(!url.query_pairs().any(|(k, _)| k == "session"));
    }

    #[test]
    fn application_id_prefix_names_its_environment() {
        assert_eq!(app_env_of("sq0idp-xyz"), SquareEnv::Production);
        assert_eq!(app_env_of("sandbox-sq0idb-xyz"), SquareEnv::Sandbox);
    }

    #[test]
    fn refreshes_a_week_after_issue_not_before() {
        let now = Utc::now();
        assert!(!refresh_due(now + chrono::Duration::days(29), now));
        assert!(!refresh_due(now + chrono::Duration::days(24), now));
        assert!(refresh_due(now + chrono::Duration::days(22), now));
        assert!(refresh_due(now - chrono::Duration::days(1), now));
    }

    #[test]
    fn scope_gaps_finds_missing_and_extra() {
        let mut granted: Vec<String> = SCOPES.iter().map(|s| s.to_string()).collect();
        assert_eq!(scope_gaps(&granted), (Vec::<String>::new(), Vec::<String>::new()));
        granted.retain(|s| s != "PAYMENTS_READ");
        granted.push("BANK_ACCOUNTS_READ".into());
        let (missing, extra) = scope_gaps(&granted);
        assert_eq!(missing, vec!["PAYMENTS_READ"]);
        assert_eq!(extra, vec!["BANK_ACCOUNTS_READ"]);
    }

    #[test]
    fn state_is_single_use_and_bound_to_the_admin() {
        let states = PendingStates::default();
        let alice = Uuid::new_v4();
        let bob = Uuid::new_v4();

        let s = states.issue(alice);
        assert!(!states.take(&s, bob), "another admin can't use it");
        assert!(!states.take(&s, alice), "and that look used it up");

        let s = states.issue(alice);
        assert!(states.take(&s, alice));
        assert!(!states.take(&s, alice), "single use");
        assert!(!states.take("made-up", alice));
    }

    #[test]
    fn token_file_round_trips_privately_per_environment() {
        let dir = std::env::temp_dir().join(format!("bb-oauth-{}", Uuid::new_v4()));
        store_in(&dir, SquareEnv::Production, &token("production")).unwrap();

        let loaded = load_from(&dir, SquareEnv::Production).unwrap().unwrap();
        assert_eq!(loaded.access_token, "EAAA-access");
        assert!(load_from(&dir, SquareEnv::Sandbox).unwrap().is_none());

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(token_path(&dir, SquareEnv::Production))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        remove_from(&dir, SquareEnv::Production).unwrap();
        assert!(load_from(&dir, SquareEnv::Production).unwrap().is_none());
        remove_from(&dir, SquareEnv::Production).unwrap(); // already gone is fine
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_token_file_for_the_other_environment_is_refused() {
        let dir = std::env::temp_dir().join(format!("bb-oauth-{}", Uuid::new_v4()));
        store_in(&dir, SquareEnv::Production, &token("sandbox")).unwrap();
        assert!(load_from(&dir, SquareEnv::Production).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn status_never_contains_a_token() {
        let cell: TokenCell = Arc::new(RwLock::new(None));
        let conn = Connection::new(
            OAuthSetup::Ready(cfg(SquareEnv::Production)),
            Some(cell.clone()),
            false,
        );
        assert_eq!(conn.status()["method"], "none");
        conn.install(Some(token("production")));
        assert_eq!(cell.read().unwrap().as_deref(), Some("EAAA-access"));

        let status = conn.status().to_string();
        assert!(status.contains("\"method\":\"oauth\""));
        assert!(!status.contains("EAAA-access"));
        assert!(!status.contains("EQAA-refresh"));
        assert!(!status.contains("sq0csp-secret"));
    }
}
