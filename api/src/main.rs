mod auth;
mod db;
mod error;
mod models;
mod routes;
mod squarespace;
mod sync;

use axum::Json;
use serde_json::{json, Value};
use sqlx::PgPool;
use std::net::SocketAddr;
use std::sync::Arc;

use crate::squarespace::Squarespace;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub squarespace: Arc<dyn Squarespace>,
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

    let db = db::connect(&database_url)
        .await
        .expect("failed to connect to database or run migrations");
    tracing::info!("database connected and migrations applied");

    let state = AppState {
        db,
        squarespace: squarespace::from_env(),
    };

    // Drain the Squarespace outbox in the background for the life of the process.
    tokio::spawn(sync::run_worker(state.clone()));

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
