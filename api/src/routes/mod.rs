pub mod admin;
pub mod bundles;
pub mod carts;
pub mod deletions;
pub mod email_admin;
pub mod customer_portal;
pub mod customers;
pub mod employees;
pub mod ingredients;
pub mod intake;
pub mod mixes;
pub mod notifications;
pub mod orders;
pub mod public;
pub mod reconciliation;
pub mod scents;
pub mod session;
pub mod settings;
pub mod square_webhooks;
pub mod sync;

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
        // Package deals: any employee can read them (intake needs the list),
        // admins define them.
        .route("/api/bundles", get(bundles::list).post(bundles::create))
        .route(
            "/api/bundles/:id",
            patch(bundles::update).delete(bundles::delete),
        )
        // Admin deletion. Everything here refuses to remove anything money has
        // touched — see the module docs.
        .route("/api/customers/:id/deletion-impact", get(deletions::customer_impact))
        .route("/api/customers/:id/delete", post(deletions::delete_customer))
        .route("/api/mixes/:id/delete", post(deletions::delete_mix))
        .route("/api/orders/:id/delete", post(deletions::delete_order))
        .route("/api/orders/:id/fulfil", post(orders::fulfil))
        // Outbound email. Relay credentials stay in the environment; only the
        // sender identity and toggles are settable here.
        .route(
            "/api/email/settings",
            get(email_admin::get).patch(email_admin::update),
        )
        .route("/api/email/test", post(email_admin::test))
        // The service-account key is written to a file, never a DB column, so it
        // stays out of the downloadable pg_dump backup.
        .route(
            "/api/email/google",
            post(email_admin::connect_google).delete(email_admin::disconnect_google),
        )
        .route("/api/email/recent", get(email_admin::recent))
        .route("/api/ingredients/:id/delete", post(deletions::delete_ingredient))
        .route("/api/scents/:id/delete", post(deletions::delete_scent))
        // Carts + Square checkout. Money only ever moves on Square's side; these
        // routes create the cart, hand it over, and read the result back.
        .route("/api/carts", get(carts::list).post(carts::create))
        .route("/api/carts/:id", get(carts::get))
        .route("/api/carts/:id/checkout", post(carts::checkout))
        .route("/api/carts/:id/checkout.svg", get(carts::checkout_qr))
        .route("/api/carts/:id/refresh", post(carts::refresh))
        .route("/api/carts/:id/cancel", post(carts::cancel))
        .route("/api/square/status", get(reconciliation::status))
        .route("/api/square/reconcile", get(reconciliation::reconcile))
        .route("/api/square/reconcile/history", get(reconciliation::history))
        .route("/api/square/events", get(square_webhooks::recent))
        // Chat notifications for customer-triggered events (admin-only).
        .route(
            "/api/notifications/targets",
            get(notifications::list).post(notifications::create),
        )
        .route(
            "/api/notifications/targets/:id",
            patch(notifications::update).delete(notifications::delete),
        )
        .route("/api/notifications/targets/:id/test", post(notifications::test))
        .route("/api/notifications/recent", get(notifications::recent))
        .route("/api/sync/status", get(sync::status))
        .route("/api/sync/retry", post(sync::retry))
        .route("/api/admin/backup", get(admin::backup))
        .route("/api/settings", get(settings::get).patch(settings::update))
        .route("/api/employees", get(employees::list).post(employees::create))
        .route("/api/employees/:id", patch(employees::update))
        .route("/api/employees/:id/reset-password", post(employees::reset_password))
        .route("/api/employees/:id/reset-mfa", post(employees::reset_mfa))
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
        .route("/api/auth/change-password", post(session::change_password))
        .route("/api/auth/me", get(session::me));

    // Customer portal — cookie-based (its own customer session), so also outside
    // the employee middleware.
    let customer_flow = Router::new()
        .route("/api/customer/login", post(customer_portal::request_link))
        .route("/api/customer/verify", post(customer_portal::verify))
        .route("/api/customer/me", get(customer_portal::me))
        .route("/api/customer/logout", post(customer_portal::logout))
        .route("/api/customer/history", get(customer_portal::history))
        .route("/api/customer/reorder", post(customer_portal::reorder));

    // Public share targets (no auth): scent view (names, no amounts) + QR.
    let public_routes = Router::new()
        .route("/api/public/scent/:id", get(public::scent))
        .route("/api/public/scent/:id/qr", get(public::scent_qr))
        // Anonymous checkout from a share link. Rate-limited, server-priced, and
        // disabled unless Square is live — see the handler docs.
        .route("/api/public/checkout", post(public::checkout));

    Router::new()
        .route("/api/health", get(crate::health))
        // Public but HMAC-verified — Square can't present an employee session.
        .route("/api/webhooks/square", post(square_webhooks::receive))
        .merge(auth_flow)
        .merge(customer_flow)
        .merge(public_routes)
        .merge(authed)
        .with_state(state)
}
