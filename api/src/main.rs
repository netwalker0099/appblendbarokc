mod audit;
mod auth;
mod backup;
mod billing;
mod customer_auth;
mod db;
mod email;
mod employee_auth;
mod error;
mod google;
mod models;
mod notify;
mod pricing;
mod ratelimit;
mod referrals;
mod routes;
mod square;
mod sync;

use axum::Json;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::email::Mailer;
use crate::square::Square;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub square: Arc<dyn Square>,
    /// Square's webhook signature key. `None` disables the receiver (it returns
    /// 503 rather than trust an unverified request that can mark carts paid).
    pub square_webhook_key: Option<Arc<str>>,
    /// The notification URL exactly as configured in the Square dashboard.
    /// Square signs `url || body`, so this must match byte-for-byte. It is
    /// configured rather than derived from request headers on purpose: `Host` is
    /// attacker-controlled, and deriving it would let a caller choose the string
    /// their forged signature was computed over.
    pub square_webhook_url: Option<Arc<str>>,
    /// Guards the one unauthenticated endpoint that writes rows and calls Square
    /// (the share-page checkout).
    pub public_checkout_limiter: Arc<ratelimit::RateLimiter>,
    /// Outbound email. Swappable at runtime so an admin can connect Google from
    /// the browser without a restart; read through [`AppState::mailer`].
    pub mailer: Arc<std::sync::RwLock<Arc<dyn Mailer>>>,
}

impl AppState {
    /// The current mailer. Cloned out under a short read lock — never hold the
    /// lock across an await.
    pub fn mailer(&self) -> Arc<dyn Mailer> {
        self.mailer.read().expect("mailer lock poisoned").clone()
    }

    /// Replace the mailer after credentials change.
    pub fn set_mailer(&self, mailer: Arc<dyn Mailer>) {
        *self.mailer.write().expect("mailer lock poisoned") = mailer;
    }
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");

    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("issue-device-token") {
        let label = args
            .get(2)
            .expect("usage: blendbar-api issue-device-token <label>");
        let pool = db::connect(&database_url)
            .await
            .expect("failed to connect to database or run migrations");
        let token = auth::issue_device_token(&pool, label)
            .await
            .expect("failed to issue device token");
        println!("Device token for \"{label}\" (store this now, it will not be shown again):");
        println!("{token}");
        return;
    }

    if args.get(1).map(String::as_str) == Some("create-admin") {
        let email = args
            .get(2)
            .expect("usage: blendbar-api create-admin <email>")
            .trim()
            .to_lowercase();
        let pool = db::connect(&database_url)
            .await
            .expect("failed to connect to database or run migrations");
        let temp = employee_auth::generate_temp_password();
        let hash = employee_auth::hash_password(&temp).expect("failed to hash password");
        let result = sqlx::query(
            "insert into employees (email, password_hash, role) values ($1, $2, 'admin')",
        )
        .bind(&email)
        .bind(&hash)
        .execute(&pool)
        .await;
        match result {
            Ok(_) => {
                println!("Admin account created: {email}");
                println!("Temporary password (shown once): {temp}");
                println!("Sign in with these, then you'll be prompted to set up MFA.");
            }
            Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                eprintln!("An account for {email} already exists.");
                std::process::exit(1);
            }
            Err(e) => panic!("failed to create admin: {e}"),
        }
        return;
    }

    let db = db::connect(&database_url)
        .await
        .expect("failed to connect to database or run migrations");
    tracing::info!("database connected and migrations applied");

    let env_opt = |key: &str| -> Option<Arc<str>> {
        std::env::var(key)
            .ok()
            .filter(|s| !s.trim().is_empty())
            .map(|s| Arc::from(s.trim()))
    };

    let square_webhook_key = env_opt("SQUARE_WEBHOOK_SIGNATURE_KEY");
    let square_webhook_url = env_opt("SQUARE_WEBHOOK_URL");

    // Both halves are required: the key proves the request came from Square, and
    // the URL is part of the signed message. One without the other cannot verify
    // anything, so the receiver stays off rather than half-on.
    match (&square_webhook_key, &square_webhook_url) {
        (Some(_), Some(url)) => {
            tracing::info!(%url, "square webhook receiver enabled")
        }
        _ => tracing::warn!(
            have_key = square_webhook_key.is_some(),
            have_url = square_webhook_url.is_some(),
            "square webhook receiver disabled (returns 503) — set both \
             SQUARE_WEBHOOK_SIGNATURE_KEY and SQUARE_WEBHOOK_URL. Until then, \
             paid carts must be settled with the 'Refresh from Square' action."
        ),
    }

    let state = AppState {
        db,
        square: square::from_env(),
        square_webhook_key,
        square_webhook_url,
        // Ten checkout attempts per IP per five minutes: far more than a real
        // buyer needs (pick a size, tap Buy), far less than is useful for
        // stuffing the database or Square's API.
        public_checkout_limiter: Arc::new(ratelimit::RateLimiter::new(
            10,
            std::time::Duration::from_secs(300),
        )),
        mailer: Arc::new(std::sync::RwLock::new(email::from_env())),
    };

    // Push contacts to Square Customers and expire abandoned checkouts, for the
    // life of the process.
    tokio::spawn(sync::run_worker(state.clone()));
    tokio::spawn(backup::run_worker(state.clone()));

    let app = routes::build_router(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], 8080));
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .expect("failed to bind 0.0.0.0:8080");

    tracing::info!("listening on {addr}");
    axum::serve(listener, app).await.expect("server error");
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}
