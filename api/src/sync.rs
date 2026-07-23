//! The outbox worker: drains `sync_outbox`, pushes each entity through the
//! configured [`Squarespace`](crate::squarespace::Squarespace) backend, writes
//! the returned id back onto the entity, and settles the job. Delivery is
//! at-least-once — a crash between a successful push and its write-back re-runs
//! the job, so `sync_order` guards against creating a duplicate order.

use std::time::Duration;

use uuid::Uuid;

use crate::models::customer::Customer;
use crate::models::order::{BottleSize, Order, OrderStatus, OrderType};
use crate::models::sync::{SyncEntity, SyncJob};
use crate::squarespace::{ContactPush, OrderPush, SyncError};
use crate::AppState;

/// Give up (mark `failed`) after this many attempts on a retryable error.
const MAX_ATTEMPTS: i32 = 6;
const POLL_INTERVAL: Duration = Duration::from_secs(5);
const BATCH: i64 = 20;

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
        "sync worker started (backend: {})",
        state.squarespace.name()
    );
    loop {
        if let Err(e) = drain_once(&state).await {
            tracing::error!("sync worker poll failed: {e}");
        }
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
        SyncEntity::Order => sync_order(state, job.entity_id).await,
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

async fn sync_contact(state: &AppState, id: Uuid) -> Result<(), SyncError> {
    let customer = sqlx::query_as::<_, Customer>("select * from customers where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| SyncError::Transport(e.to_string()))?
        .ok_or_else(|| SyncError::Config(format!("customer {id} no longer exists")))?;

    let push = ContactPush {
        id: customer.id,
        email: customer.email.clone(),
        name: customer.name.clone(),
        marketing_consent: customer.marketing_consent,
    };
    let contact_id = state.squarespace.upsert_contact(&push).await?;

    sqlx::query("update customers set squarespace_contact_id = $1 where id = $2")
        .bind(&contact_id)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| SyncError::Transport(e.to_string()))?;
    Ok(())
}

async fn sync_order(state: &AppState, id: Uuid) -> Result<(), SyncError> {
    let order = sqlx::query_as::<_, Order>("select * from orders where id = $1")
        .bind(id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| SyncError::Transport(e.to_string()))?
        .ok_or_else(|| SyncError::Config(format!("order {id} no longer exists")))?;

    // Order creation is not idempotent on Squarespace's side. If a prior attempt
    // already got an id back (and only the job write-back failed), don't create a
    // second order — treat the job as done.
    if order.squarespace_order_id.is_some() {
        return Ok(());
    }

    let customer =
        sqlx::query_as::<_, Customer>("select * from customers where id = $1")
            .bind(order.customer_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| SyncError::Transport(e.to_string()))?
            .ok_or_else(|| SyncError::Config(format!("customer {} no longer exists", order.customer_id)))?;

    let push = OrderPush {
        id: order.id,
        email: customer.email.clone(),
        name: customer.name.clone(),
        order_type: order_type_label(order.order_type).to_string(),
        size: size_label(order.size).to_string(),
        status: status_label(order.status).to_string(),
        amount: order.amount,
        description: format!(
            "{} ({})",
            order_type_label(order.order_type),
            size_label(order.size)
        ),
    };
    let order_id = state.squarespace.create_order(&push).await?;

    sqlx::query("update orders set squarespace_order_id = $1 where id = $2")
        .bind(&order_id)
        .bind(id)
        .execute(&state.db)
        .await
        .map_err(|e| SyncError::Transport(e.to_string()))?;
    Ok(())
}

fn order_type_label(t: OrderType) -> &'static str {
    match t {
        OrderType::SetPerfume => "Set perfume",
        OrderType::CustomMix => "Custom mix",
    }
}

fn size_label(s: BottleSize) -> &'static str {
    match s {
        BottleSize::Oz3_4 => "3.4 oz",
        BottleSize::Oz1_7 => "1.7 oz",
        BottleSize::Roller => "Roller",
    }
}

fn status_label(s: OrderStatus) -> &'static str {
    match s {
        OrderStatus::Lead => "lead",
        OrderStatus::Paid => "paid",
        OrderStatus::Fulfilled => "fulfilled",
    }
}
