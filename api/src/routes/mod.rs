pub mod admin;
pub mod audit_log;
pub mod backup_admin;
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
pub mod restore;
pub mod scents;
pub mod session;
pub mod settings;
pub mod square_webhooks;
pub mod sync;

use axum::extract::DefaultBodyLimit;
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
        // Scheduled backups. Admin-only throughout (the `AdminEmployee`
        // extractor on each handler): these configure and can trigger a full
        // export of every customer record.
        .route("/api/admin/backup/status", get(backup_admin::status))
        .route("/api/admin/backup/passphrase", post(backup_admin::set_passphrase))
        .route(
            "/api/admin/backup/destinations",
            get(backup_admin::list).post(backup_admin::create),
        )
        .route(
            "/api/admin/backup/destinations/:id",
            patch(backup_admin::update).delete(backup_admin::delete),
        )
        .route(
            "/api/admin/backup/destinations/:id/run",
            post(backup_admin::run_now),
        )
        .route("/api/admin/backup/runs", get(backup_admin::runs))
        // Upload a backup. Without the confirmation header this only inspects,
        // so destruction is opt-in and a malformed request cannot cause it. The
        // body limit is raised well past the default 2MB: this is a whole
        // database, and a silently truncated upload is the worst possible input
        // to a restore.
        .route(
            "/api/admin/backup/restore",
            post(restore::upload).layer(DefaultBodyLimit::max(512 * 1024 * 1024)),
        )
        .route("/api/admin/backup/safety-copies", get(restore::safety_copies))
        .route(
            "/api/admin/backup/safety-copies/:name",
            get(restore::download_safety_copy),
        )
        // Read-only views onto the audit log. There is deliberately no handler
        // that writes or deletes one — the database refuses those anyway.
        .route("/api/admin/audit", get(audit_log::list))
        .route("/api/admin/audit/verify", get(audit_log::verify))
        .route("/api/admin/audit/segments", get(audit_log::segments))
        // Archiving is a POST because it changes things — and, like every other
        // mutation, the act of archiving is itself written to the audit log.
        .route("/api/admin/audit/archive", post(audit_log::archive_now))
        .route("/api/settings", get(settings::get).patch(settings::update))
        .route("/api/employees", get(employees::list).post(employees::create))
        .route("/api/employees/:id", patch(employees::update))
        .route("/api/employees/:id/reset-password", post(employees::reset_password))
        .route("/api/employees/:id/reset-mfa", post(employees::reset_mfa))
        // Order matters. Layers are applied inside-out, so this reads bottom-up:
        // `require_employee` runs first and injects the actor, then `audit`
        // records the action. Audit has to be *inside* auth — outside it, there
        // would be nobody to attribute the entry to.
        //
        // Audit sits on the whole authenticated router rather than on individual
        // handlers so a route added later is covered automatically. The failure
        // mode of an audit log is the entry nobody wrote.
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            crate::audit::record,
        ))
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
