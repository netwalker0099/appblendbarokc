//! Where a backup goes.
//!
//! One trait, so the scheduler does not know or care whether a backup is being
//! uploaded or emailed, and so a third destination is a new file rather than an
//! edit to the worker.
//!
//! ## On email as a backup destination
//!
//! It is supported because it is genuinely useful — it needs no cloud setup and
//! it lands somewhere a person actually looks — but it is the weakest of the
//! options and the code says so rather than pretending otherwise:
//!
//!   * Providers cap attachments (Gmail at 25MB). A database that grows past
//!     that stops being backed up, so the size is checked *before* sending and
//!     the failure names the real cause.
//!   * There is no delete, so retention cannot apply. Copies accumulate in a
//!     mailbox forever.
//!   * The mailbox becomes as sensitive as the database. That is survivable only
//!     because the attachment is encrypted before it leaves this box.
//!
//! Drive is the better primary. Email is a good second copy.

use async_trait::async_trait;

use super::drive::DriveBackend;
use super::{Artifact, BackupError, DestinationRow};
use crate::email::{credentials, gmail, Attached, Outgoing};
use crate::AppState;

/// Gmail's attachment ceiling is 25MB; base64 in transit costs about a third on
/// top, so the real limit on raw bytes is nearer 18MB. Refuse a bit below that
/// rather than hand a message to the relay that it will bounce.
const MAX_EMAIL_BYTES: usize = 18 * 1024 * 1024;

#[async_trait]
pub trait Backend: Send + Sync {
    /// Whether [`Backend::delete`] does anything. False for email, which is why
    /// retention silently does not apply there.
    fn supports_delete(&self) -> bool;

    /// Send the artefact. Returns the destination's own id for it, when there is
    /// one — that id is what retention later deletes.
    async fn upload(&self, artifact: &Artifact) -> Result<Option<String>, BackupError>;

    async fn delete(&self, remote_id: &str) -> Result<(), BackupError>;
}

/// Check a destination's shape without touching credentials, the network or the
/// database.
///
/// Called when a destination is saved, not only when it runs. A recipient
/// address left blank should be refused by the form that omitted it, rather than
/// discovered as a red row at 2am — and by then the backup for that night has
/// already not happened.
pub fn validate(kind: &str, config: &serde_json::Value) -> Result<(), BackupError> {
    match kind {
        "google_drive" => Ok(()),
        "email" => {
            let to = config
                .get("to")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .unwrap_or("");
            if to.is_empty() {
                return Err(BackupError::NotConfigured(
                    "this destination has no recipient address".into(),
                ));
            }
            // Not full RFC validation — just enough to catch the typo that would
            // otherwise be found by the relay, one night later.
            if !to.contains('@') || to.starts_with('@') || to.ends_with('@') {
                return Err(BackupError::NotConfigured(format!(
                    "'{to}' is not an email address"
                )));
            }
            Ok(())
        }
        "sharepoint" => Err(BackupError::NotConfigured(
            "SharePoint backups are not implemented — this deployment has no Microsoft \
             365 tenant. Use Google Drive or email."
                .into(),
        )),
        other => Err(BackupError::NotConfigured(format!(
            "unknown destination type '{other}'"
        ))),
    }
}

/// Build the backend for a destination row.
pub async fn build(
    state: &AppState,
    dest: &DestinationRow,
) -> Result<Box<dyn Backend>, BackupError> {
    validate(&dest.kind, &dest.config)?;

    match dest.kind.as_str() {
        "google_drive" => {
            let account = load_service_account()?;
            // Per-destination override, else the mailbox email already sends as.
            let impersonate = dest
                .config
                .get("impersonate")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    std::env::var("GOOGLE_IMPERSONATE")
                        .ok()
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                })
                .unwrap_or_default();

            let folder_id = dest
                .config
                .get("folder_id")
                .and_then(|v| v.as_str())
                .map(String::from);

            Ok(Box::new(DriveBackend::new(account, impersonate, folder_id)?))
        }

        "email" => {
            let to = dest
                .config
                .get("to")
                .and_then(|v| v.as_str())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    BackupError::NotConfigured("this destination has no recipient address".into())
                })?;

            Ok(Box::new(EmailBackend {
                state: state.clone(),
                to,
            }))
        }

        // `validate` above has already rejected everything else — sharepoint
        // included, which is named in the schema so a backend can be added later
        // without a migration.
        other => Err(BackupError::NotConfigured(format!(
            "unknown destination type '{other}'"
        ))),
    }
}

/// The service-account key, from the environment or the one an admin uploaded
/// for email. Shared deliberately: making someone upload the same key twice is
/// two things to rotate and one of them will be forgotten.
fn load_service_account() -> Result<gmail::ServiceAccount, BackupError> {
    gmail::load_service_account()
        .or_else(credentials::load_stored)
        .ok_or_else(|| {
            BackupError::NotConfigured(
                "no Google service-account key is configured — connect Google under \
                 Admin → Email first; Drive backups reuse that key."
                    .into(),
            )
        })?
        .map_err(|e| BackupError::NotConfigured(e.to_string()))
}

// --- Email ------------------------------------------------------------------

struct EmailBackend {
    state: AppState,
    to: String,
}

#[async_trait]
impl Backend for EmailBackend {
    fn supports_delete(&self) -> bool {
        false
    }

    async fn upload(&self, artifact: &Artifact) -> Result<Option<String>, BackupError> {
        if artifact.bytes.len() > MAX_EMAIL_BYTES {
            return Err(BackupError::Rejected(format!(
                "the backup is {:.1}MB, over the {}MB this can attach. The database has \
                 outgrown email as a destination — use Google Drive.",
                artifact.bytes.len() as f64 / 1_048_576.0,
                MAX_EMAIL_BYTES / 1_048_576,
            )));
        }

        let mailer = self.state.mailer();
        if !mailer.is_live() {
            // The mock logs and reports success, which for a backup would mean a
            // green history and no backups anywhere. Refuse instead.
            return Err(BackupError::NotConfigured(
                "email is not configured — connect Google or an SMTP relay under \
                 Admin → Email. Nothing was sent."
                    .into(),
            ));
        }

        let sender = crate::email::dispatch::load_sender(&self.state.db)
            .await
            .map_err(|e| BackupError::NotConfigured(e.to_string()))?;

        let body = crate::email::templates::backup_ready(
            &artifact.filename,
            artifact.bytes.len(),
            artifact.plain_bytes,
        );

        mailer
            .send(Outgoing {
                to: self.to.clone(),
                from_address: sender.from_address,
                from_name: sender.from_name,
                reply_to: sender.reply_to,
                body,
                attachments: vec![Attached {
                    filename: artifact.filename.clone(),
                    bytes: artifact.bytes.clone(),
                    content_type: "application/octet-stream".into(),
                }],
            })
            .await
            .map_err(|e| match e {
                crate::email::MailError::Transport(m) => BackupError::Transport(m),
                crate::email::MailError::NotConfigured(m) => BackupError::NotConfigured(m),
                crate::email::MailError::Rejected(m) => BackupError::Rejected(m),
            })?;

        // No id: a sent email cannot be recalled, which is why retention does
        // not apply to this destination.
        Ok(None)
    }

    async fn delete(&self, _remote_id: &str) -> Result<(), BackupError> {
        Err(BackupError::Rejected(
            "email backups cannot be deleted once sent".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_email_size_ceiling_leaves_room_for_base64_overhead() {
        // A 25MB raw attachment becomes ~34MB on the wire and bounces, so the
        // limit has to be the pre-encoding size.
        assert!(MAX_EMAIL_BYTES < 25 * 1024 * 1024 * 3 / 4);
    }

    #[test]
    fn an_email_destination_needs_a_plausible_recipient() {
        assert!(validate("email", &json!({"to": "owner@theblendbarokc.com"})).is_ok());
        assert!(validate("email", &json!({})).is_err());
        assert!(validate("email", &json!({"to": "  "})).is_err());
        assert!(validate("email", &json!({"to": "not-an-address"})).is_err());
        assert!(validate("email", &json!({"to": "@nope"})).is_err());
    }

    #[test]
    fn drive_needs_nothing_up_front() {
        // Folder and impersonation both have sensible fallbacks, so an empty
        // config is a valid "my Drive, default mailbox".
        assert!(validate("google_drive", &json!({})).is_ok());
    }

    #[test]
    fn sharepoint_refuses_clearly_instead_of_pretending() {
        // The kind is accepted by the schema. What must not happen is a
        // destination that looks configured and silently backs nothing up,
        // leaving someone believing they have a second copy.
        let err = validate("sharepoint", &json!({})).unwrap_err();
        assert!(err.to_string().contains("not implemented"));
    }

    #[test]
    fn unknown_kinds_are_refused() {
        assert!(validate("dropbox", &json!({})).is_err());
    }
}
