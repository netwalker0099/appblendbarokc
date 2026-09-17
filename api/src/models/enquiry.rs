use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use uuid::Uuid;

/// An event booking enquiry submitted from the public site.
///
/// This is the record the business works from: the notification that goes to
/// chat is a prompt to come and look at one of these, never a replacement for
/// it. Contact details live here and are only mirrored into a chat channel when
/// a target opts in.
#[derive(Debug, Serialize, sqlx::FromRow)]
pub struct EventEnquiry {
    pub id: Uuid,
    pub first_name: String,
    pub last_name: String,
    pub email: String,
    pub phone: String,
    pub event_date: NaiveDate,
    pub details: String,
    pub status: String,
    pub staff_notes: Option<String>,
    pub handled_by: Option<Uuid>,
    pub handled_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// The states an enquiry moves through. Small on purpose — this is a list
/// someone works down, not a sales pipeline.
pub const STATUSES: &[&str] = &["new", "contacted", "booked", "declined"];

pub fn is_valid_status(s: &str) -> bool {
    STATUSES.contains(&s)
}

impl EventEnquiry {
    pub fn full_name(&self) -> String {
        format!("{} {}", self.first_name, self.last_name)
    }
}
