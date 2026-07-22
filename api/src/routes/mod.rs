pub mod customers;
pub mod ingredients;
pub mod intake;
pub mod mixes;
pub mod orders;
pub mod scents;

use axum::middleware;
use axum::routing::{get, patch, post};
use axum::Router;

use crate::{auth, AppState};

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
        .route(
            "/api/ingredients",
            get(ingredients::list).post(ingredients::create),
        )
        .route("/api/ingredients/:id", patch(ingredients::update))
        .route("/api/scents", get(scents::list).post(scents::create))
        .route("/api/scents/:id", patch(scents::update))
        .route("/api/mixes/:id", get(mixes::get).patch(mixes::update))
        .route("/api/orders", get(orders::list))
        .route("/api/orders/:id", get(orders::get).patch(orders::update))
        .route("/api/intake", post(intake::intake))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_operator_token,
        ));

    Router::new()
        .route("/api/health", get(crate::health))
        .merge(authed)
        .with_state(state)
}
