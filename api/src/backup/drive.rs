//! Uploading backups to Google Drive.
//!
//! ## Why it impersonates a person
//!
//! A service account is not a user and, under current Workspace policy, has no
//! Drive storage quota of its own. Uploading *as* the service account fails with
//! `storageQuotaExceeded` — a confusing error for something that looks like a
//! permissions problem. So the upload runs under domain-wide delegation as a
//! real mailbox, exactly as the Gmail sender does, and the file lands in that
//! person's Drive against their quota where they can actually see it.
//!
//! ## Scope
//!
//! Only `drive.file`: files this application created. It cannot read, list or
//! touch anything else in the person's Drive, which matters when the credential
//! sits on an internet-facing VPS. It is also exactly enough to delete its own
//! old backups, which is what makes retention possible.
//!
//! The Workspace admin has to authorise this scope for the service account's
//! client id — the same one-off step as `gmail.send`, and a *separate* entry.
//! Adding Drive to a delegation that already has Gmail is the step people miss,
//! so the token error names it.
//!
//! ## Resumable, not multipart
//!
//! Drive's simple multipart upload wants the whole body in one request and is
//! documented for small files. Backups only grow, and the failure mode of
//! outgrowing it is an upload that starts failing one night with no code change.
//! The resumable protocol costs one extra round trip and has no such ceiling.

use async_trait::async_trait;

use super::destination::Backend;
use super::{Artifact, BackupError};
use crate::email::gmail::ServiceAccount;
use crate::google::{summarise, TokenSource};

const SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
const RESUMABLE_URL: &str = "https://www.googleapis.com/upload/drive/v3/files?uploadType=resumable";
const FILES_URL: &str = "https://www.googleapis.com/drive/v3/files";

pub struct DriveBackend {
    tokens: TokenSource,
    /// Optional parent folder. Without one the file lands in the impersonated
    /// user's My Drive root.
    folder_id: Option<String>,
}

impl DriveBackend {
    pub fn new(
        account: ServiceAccount,
        impersonate: String,
        folder_id: Option<String>,
    ) -> Result<Self, BackupError> {
        if impersonate.trim().is_empty() {
            return Err(BackupError::NotConfigured(
                "Google Drive backups need a Workspace user to upload as — a service \
                 account has no Drive of its own. Set one on the destination, or set \
                 GOOGLE_IMPERSONATE."
                    .into(),
            ));
        }
        let tokens = TokenSource::new(account, impersonate, SCOPE)
            .map_err(|e| BackupError::NotConfigured(e.to_string()))?;
        Ok(Self {
            tokens,
            folder_id: folder_id.filter(|f| !f.trim().is_empty()),
        })
    }
}

#[async_trait]
impl Backend for DriveBackend {
    fn supports_delete(&self) -> bool {
        true
    }

    async fn upload(&self, artifact: &Artifact) -> Result<Option<String>, BackupError> {
        let token = self
            .tokens
            .access_token()
            .await
            .map_err(|e| BackupError::NotConfigured(e.to_string()))?;

        let mut metadata = serde_json::json!({
            "name": artifact.filename,
            // Not a MIME type Drive will try to be clever about. An .age file is
            // opaque bytes and must come back out byte-identical.
            "mimeType": "application/octet-stream",
        });
        if let Some(folder) = &self.folder_id {
            metadata["parents"] = serde_json::json!([folder]);
        }

        // Step 1: start the session. Drive replies with a one-shot upload URL.
        let start = self
            .tokens
            .http()
            .post(RESUMABLE_URL)
            .bearer_auth(&token)
            .header("X-Upload-Content-Type", "application/octet-stream")
            .header("X-Upload-Content-Length", artifact.bytes.len().to_string())
            .json(&metadata)
            .send()
            .await
            .map_err(|e| BackupError::Transport(format!("could not reach Google Drive: {e}")))?;

        let status = start.status();
        if !status.is_success() {
            let body = start.text().await.unwrap_or_default();
            return Err(classify(status, &summarise(&body), self.folder_id.as_deref()));
        }

        let location = start
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| {
                BackupError::Transport(
                    "Drive accepted the upload session but returned no Location header".into(),
                )
            })?
            .to_string();

        // Step 2: send the bytes. One PUT — the artefact is already in memory, so
        // chunking would buy nothing but complexity.
        let finish = self
            .tokens
            .http()
            .put(&location)
            .bearer_auth(&token)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .body(artifact.bytes.clone())
            .send()
            .await
            .map_err(|e| BackupError::Transport(format!("upload failed: {e}")))?;

        let status = finish.status();
        let body = finish.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(classify(status, &summarise(&body), self.folder_id.as_deref()));
        }

        // The file id is what makes retention possible later.
        let id = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("id").and_then(|i| i.as_str()).map(String::from));

        Ok(id)
    }

    async fn delete(&self, remote_id: &str) -> Result<(), BackupError> {
        let token = self
            .tokens
            .access_token()
            .await
            .map_err(|e| BackupError::NotConfigured(e.to_string()))?;

        let resp = self
            .tokens
            .http()
            .delete(format!("{FILES_URL}/{remote_id}"))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(|e| BackupError::Transport(format!("delete failed: {e}")))?;

        let status = resp.status();
        // Already gone is the outcome we wanted. Somebody tidying the folder by
        // hand must not leave retention permanently stuck retrying.
        if status.is_success() || status.as_u16() == 404 {
            return Ok(());
        }
        let body = resp.text().await.unwrap_or_default();
        Err(classify(status, &summarise(&body), None))
    }
}

/// Turn an HTTP status into an error that says what to do about it.
fn classify(status: reqwest::StatusCode, detail: &str, folder: Option<&str>) -> BackupError {
    match status.as_u16() {
        // Worth another attempt on the next scheduled run.
        429 | 500..=599 => BackupError::Transport(format!("Google Drive said {status}: {detail}")),
        403 if detail.contains("storageQuota") => BackupError::Rejected(format!(
            "the Drive account is out of storage ({detail}). Free space, or lower the \
             retention count so old backups are pruned sooner."
        )),
        401 | 403 => BackupError::NotConfigured(format!(
            "Google Drive refused the request ({status}: {detail}). The usual cause is \
             the service account not being authorised for {SCOPE} — Gmail and Drive are \
             separate entries under Admin → Security → API controls → Domain-wide \
             delegation, and adding the second one is easy to miss."
        )),
        404 => BackupError::NotConfigured(match folder {
            Some(f) => format!(
                "Drive folder '{f}' was not found ({detail}). With the drive.file scope the \
                 app can only use a folder it created or one explicitly shared with the \
                 user it uploads as — check the folder id and that it is shared."
            ),
            None => format!("Google Drive returned 404: {detail}"),
        }),
        _ => BackupError::Rejected(format!("Google Drive said {status}: {detail}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::StatusCode;

    #[test]
    fn transient_failures_are_transport_and_the_rest_are_not() {
        // The distinction decides whether the next run has any chance of
        // working, so it is worth pinning.
        assert!(matches!(
            classify(StatusCode::TOO_MANY_REQUESTS, "rate limit", None),
            BackupError::Transport(_)
        ));
        assert!(matches!(
            classify(StatusCode::BAD_GATEWAY, "upstream", None),
            BackupError::Transport(_)
        ));
        assert!(matches!(
            classify(StatusCode::UNAUTHORIZED, "invalid creds", None),
            BackupError::NotConfigured(_)
        ));
    }

    #[test]
    fn a_full_drive_is_named_rather_than_reported_as_a_permissions_problem() {
        let e = classify(StatusCode::FORBIDDEN, "storageQuotaExceeded", None);
        assert!(matches!(e, BackupError::Rejected(_)));
        assert!(e.to_string().contains("out of storage"));
    }

    #[test]
    fn a_403_points_at_the_delegation_step_people_miss() {
        let e = classify(StatusCode::FORBIDDEN, "Insufficient permission", None);
        assert!(e.to_string().contains("Domain-wide"));
        assert!(e.to_string().contains("drive.file"));
    }

    #[test]
    fn a_missing_folder_explains_the_drive_file_scope() {
        let e = classify(StatusCode::NOT_FOUND, "File not found", Some("abc123"));
        assert!(e.to_string().contains("abc123"));
        assert!(e.to_string().contains("shared"));
    }

    #[test]
    fn uploading_as_nobody_is_refused_up_front() {
        // Better here than as a storageQuotaExceeded from Google an hour later.
        let account = ServiceAccount {
            client_email: "sa@project.iam.gserviceaccount.com".into(),
            private_key: "not a key".into(),
            private_key_id: String::new(),
        };
        let Err(err) = DriveBackend::new(account, "   ".into(), None) else {
            panic!("expected an empty impersonation target to be refused");
        };
        assert!(matches!(err, BackupError::NotConfigured(_)));
        assert!(err.to_string().contains("service account has no Drive"));
    }
}
