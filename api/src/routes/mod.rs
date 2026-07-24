pub mod admin;
pub mod customers;
pub mod ingredients;
pub mod intake;
pub mod mixes;
pub mod orders;
pub mod scents;
pub mod session;
pub mod sync;
pub mod webhooks;

use axum::middleware;
use axum::routing::{get, patch, post};
use axum::Router;

use crate::{employee_auth, AppState};

pub fn build_router(state: AppState) -> Router {
    let authed = Router::new()
        .route("/api/customers", get(customers::list))
        .route(
            "/api/customers/:id",
            get(customers::get).patch(customers::update),
        )
        .route(
            "/api/customers/:id/mixes",
            get(mixes::list_for_customer),
        )
        .route("/api/customers/:id/reorder", get(customers::reorder))
        .route(
            "/api/ingredients",
            get(ingredients::list).post(ingredients::create),
        )
        .route("/api/ingredients/:id", patch(ingredients::update))
        .route("/api/scents", get(scents::list).post(scents::create))
        .route("/api/scents/:id", get(scents::get).patch(scents::update))
        .route("/api/mixes/:id", get(mixes::get).patch(mixes::update))
        .route("/api/orders", get(orders::list))
        .route("/api/orders/:id", get(orders::get).patch(orders::update))
        .route("/api/intake", post(intake::intake))
        .route("/api/sync/status", get(sync::status))
        .route("/api/sync/retry", post(sync::retry))
        .route("/api/webhooks/recent", get(webhooks::recent))
        .route("/api/admin/backup", get(admin::backup))
        // Every operator route requires a full (MFA-complete) employee session;
        // admin-only routes additionally use the `AdminEmployee` extractor.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            employee_auth::require_employee,
        ));

    // Employee auth flow — cookie-based, so these manage their own session and
    // sit outside the bearer-token middleware.
    let auth_flow = Router::new()
        .route("/api/auth/login", post(session::login))
        .route("/api/auth/mfa/enroll", post(session::mfa_enroll))
        .route("/api/auth/mfa/verify", post(session::mfa_verify))
        .route("/api/auth/logout", post(session::logout))
        .route("/api/auth/me", get(session::me));

    Router::new()
        .route("/api/health", get(crate::health))
        // Public but HMAC-verified — Squarespace can't present an operator token.
        .route("/api/webhooks/squarespace", post(webhooks::receive))
        .merge(auth_flow)
        .merge(authed)
        .with_state(state)
}
