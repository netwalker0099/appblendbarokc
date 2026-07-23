use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SyncEntity {
    Contact,
    Order,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, sqlx::Type)]
#[sqlx(type_name = "text", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SyncStatus {
    Pending,
    Succeeded,
    Failed,
}

/// One row of the `sync_outbox` table — a pending/settled downstream push.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct SyncJob {
    pub id: Uuid,
    pub entity_type: SyncEntity,
    pub entity_id: Uuid,
    pub status: SyncStatus,
    pub attempts: i32,
    pub last_error: Option<String>,
    pub next_attempt_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
