use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct Customer {
    pub id: Uuid,
    pub email: String,
    pub name: Option<String>,
    pub marketing_consent: bool,
    pub marketing_consent_at: Option<DateTime<Utc>>,
    pub squarespace_contact_id: Option<String>,
    pub created_at: DateTime<Utc>,
}
