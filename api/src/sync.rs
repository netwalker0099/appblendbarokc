//! The outbox worker: drains `sync_outbox`, pushes each customer into Square
//! Customers, writes the returned id back, and settles the job.
//!
//! Only contacts flow through here. Orders used to as well, back when Squarespace
//! was the sink; under Square they reach the payment processor through cart
//! checkout instead, which is synchronous because the operator is standing there
//! waiting for a link. A background outbox is the right shape for "this should
//! eventually reach the CRM" and the wrong shape for "this customer is waiting
//! to pay".
//!
//! Delivery is at-least-once — a crash between a successful push and its
//! write-back re-runs the job — so `upsert_customer` must be idempotent on
//! Square's side, which it is (it searches by email before creating).
//!
//! The worker also expires abandoned checkouts on the same loop.

use std::time::Duration;

use uuid::Uuid;

use crate::models::customer::Customer;
use crate::models::sync::{SyncEntity, SyncJob};
use crate::square::{CustomerPush, SquareError};
use crate::AppState;

/// Give up (mark `failed`) after this many attempts on a retryable error.
const MAX_ATTEMPTS: i32 = 6;
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const BATCH: i64 = 20;

/// A payment link nobody used within this many hours is treated as abandoned.
/// Long enough that a customer can still pay after leaving the stand; short
/// enough that their blend isn't held hostage overnight.
const CHECKOUT_TTL_HOURS: i64 = 24;

/// Sweep for abandoned checkouts every N polls (~10 minutes at a 5s poll). It is
/// a tidy-up, not a hot path.
const EXPIRY_EVERY: u32 = 120;

/// Public site root, used to build links in customer email.
fn site_url() -> String {
    std::env::var("CUSTOMER_SITE_URL")
        .unwrap_or_else(|_| "https://sandbox.theblendbarokc.com".to_string())
}

/// Transactionally enqueue a downstream sync. Safe to call repeatedly — a pending
/// job for the same entity is reused (its retry clock reset to now) rather than
/// duplicated, thanks to the partial unique index on `(entity_type, entity_id)`.
pub async fn enqueue<'e, E>(exec: E, entity: SyncEntity, entity_id: Uuid) -> Result<(), sqlx::Error>
where
    E: sqlx::PgExecutor<'e>,
{
    sqlx::query(
        r#"
        insert into sync_outbox (entity_type, entity_id)
        values ($1, $2)
        on conflict (entity_type, entity_id) where status = 'pending'
        do update set next_attempt_at = now(), updated_at = now()
        "#,
    )
    .bind(entity)
    .bind(entity_id)
    .execute(exec)
    .await?;
    Ok(())
}

/// Run forever, draining due jobs on a fixed interval. Spawned once at startup.
/// Assumes a single worker (no `for update skip locked`); fine for one API process.
pub async fn run_worker(state: AppState) {
    tracing::info!(
        "sync worker started (square backend: {})",
        state.square.name()
    );
    // One client for the life of the worker, so chat webhooks reuse connections
    // instead of renegotiating TLS per message.
    let http = reqwest::Client::builder()
        .user_agent("blendbar-app/0.2")
        .build()
        .expect("failed to build notification http client");

    let mut tick: u32 = 0;
    loop {
        if let Err(e) = drain_once(&state).await {
            tracing::error!("sync worker poll failed: {e}");
        }

        // Chat notifications for customer-triggered events. Kept on the worker,
        // never on the payment path — a Discord outage must not fail a checkout.
        if let Err(e) = crate::notify::drain(&state.db, &http).await {
            tracing::error!("notification drain failed: {e}");
        }

        // Queued customer email ("your blend is ready"). Sign-in links do not
        // come through here — they go out inline, because someone is waiting.
        if let Err(e) =
            crate::email::dispatch::drain(&state.db, state.mailer.as_ref(), &site_url()).await
        {
            tracing::error!("email drain failed: {e}");
        }

        if tick % EXPIRY_EVERY == 0 {
            if let Err(e) = crate::billing::expire_stale_checkouts(
                &state.db,
                state.square.as_ref(),
                CHECKOUT_TTL_HOURS,
            )
            .await
            {
                tracing::error!("expiring stale checkouts failed: {e}");
            }
        }
        tick = tick.wrapping_add(1);

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

async fn drain_once(state: &AppState) -> Result<(), sqlx::Error> {
    let jobs = sqlx::query_as::<_, SyncJob>(
        r#"
        select * from sync_outbox
        where status = 'pending' and next_attempt_at <= now()
        order by next_attempt_at
        limit $1
        "#,
    )
    .bind(BATCH)
    .fetch_all(&state.db)
    .await?;

    for job in jobs {
        process(state, &job).await?;
    }
    Ok(())
}

async fn process(state: &AppState, job: &SyncJob) -> Result<(), sqlx::Error> {
    let result = match job.entity_type {
        SyncEntity::Contact => sync_contact(state, job.entity_id).await,
    };

    match result {
        Ok(()) => {
            sqlx::query(
                "update sync_outbox set status = 'succeeded', last_error = null, updated_at = now() where id = $1",
            )
            .bind(job.id)
            .execute(&state.db)
            .await?;
            tracing::info!(entity_id = %job.entity_id, kind = ?job.entity_type, "sync ok");
        }
        Err(err) => {
            let attempts = job.attempts + 1;
            let give_up = !err.retryable() || attempts >= MAX_ATTEMPTS;
            if give_up {
                sqlx::query(
                    "update sync_outbox set status = 'failed', attempts = $2, last_error = $3, updated_at = now() where id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(err.to_string())
                .execute(&state.db)
                .await?;
                tracing::error!(entity_id = %job.entity_id, "sync failed permanently: {err}");
            } else {
                // Exponential backoff: 10s, 20s, 40s, ... so a flaky downstream
                // isn't hammered.
                let backoff_secs = (2i64.pow(attempts as u32) * 5) as f64;
                sqlx::query(
                    "update sync_outbox set attempts = $2, last_error = $3, next_attempt_at = now() + make_interval(secs => $4), updated_at = now() where id = $1",
                )
                .bind(job.id)
                .bind(attempts)
                .bind(err.to_string())
                .bind(backoff_secs)
                .execute(&state.db)
                .await?;
                tracing::warn!(entity_id = %job.entity_id, attempts, "sync failed, will retry: {err}");
            }
        }
    }
    Ok(())
}

async fn sync_contact(state: &AppState, id: Uuid) -> Result<(), SquareError> {
    let customer = sqlx::query_as::<_, Customer>("select * from customers where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| SquareError::Transport(e.to_string()))?
        .ok_or_else(|| SquareError::Config(format!("customer {id} no longer exists")))?;

    let push = CustomerPush {
        id: customer.id,
        email: customer.email.clone(),
        name: customer.name.clone(),
        marketing_consent: customer.marketing_consent,
    };
    let square_customer_id = state.square.upsert_customer(&push).await?;

    sqlx::query("update customers set square_customer_id = $1 where id = $2")
        .bind(&square_customer_id)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| SquareError::Transport(e.to_string()))?;
    Ok(())
}
