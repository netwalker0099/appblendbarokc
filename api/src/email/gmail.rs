//! Sending through the Gmail API as a Google Workspace mailbox, authorised by a
//! **service account with domain-wide delegation**.
//!
//! ## How the authorisation works
//!
//! There is no user at a keyboard, so there is no consent screen and no refresh
//! token to go stale. Instead the server signs a JWT with the service account's
//! private key, asserting "I am this service account, acting as
//! `hello@theblendbarokc.com`, and I want `gmail.send`". Google exchanges that
//! assertion for an access token good for an hour.
//!
//! The `sub` claim is the delegation: it is what makes the call act as a real
//! mailbox rather than as the service account (which has no inbox of its own).
//! A Workspace super-admin has to authorise the service account's client id for
//! the `gmail.send` scope, once, in Admin → Security → API controls → Domain-wide
//! delegation. Nothing expires after that.
//!
//! ## Why this over 3-legged OAuth
//!
//! A user-consent refresh token can be revoked, or lapse if unused, and when it
//! does, mail stops until a human signs in again. For sign-in links — the only
//! way into the customer portal — that failure mode arrives at 2am and locks
//! customers out. A service account has no such moment.
//!
//! ## Scope
//!
//! Only `gmail.send`. It cannot read a single message. That matters: it is a
//! *sensitive* scope rather than a *restricted* one, and for an app used inside
//! its own Workspace, Google requires no verification or CASA assessment.

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use super::{MailError, Mailer, Outgoing};

const SEND_URL: &str = "https://gmail.googleapis.com/gmail/v1/users/me/messages/send";
const SCOPE: &str = "https://www.googleapis.com/auth/gmail.send";

/// The fields this needs out of a service-account key file.
#[derive(Deserialize)]
pub struct ServiceAccount {
    pub client_email: String,
    pub private_key: String,
    #[serde(default)]
    pub private_key_id: String,
}

/// Written by hand rather than derived: a derived `Debug` would print the
/// private key in full the first time anything logged this struct, or a panic
/// unwound through it.
impl std::fmt::Debug for ServiceAccount {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServiceAccount")
            .field("client_email", &self.client_email)
            .field("private_key_id", &self.private_key_id)
            .field("private_key", &"<redacted>")
            .finish()
    }
}

/// The mailer holds a token source rather than the JWT machinery itself; see
/// `crate::google`, which Drive backups share.
pub struct GmailMailer {
    tokens: crate::google::TokenSource,
}

impl GmailMailer {
    pub fn new(account: ServiceAccount, impersonate: String) -> Result<Self, MailError> {
        Ok(Self {
            tokens: crate::google::TokenSource::new(account, impersonate, SCOPE)?,
        })
    }
}

/// Shared with the Drive backup uploader, which has to explain the same class of
/// Google failure.
use crate::google::summarise;

/// Build the RFC 5322 message and base64url it, which is what `raw` wants.
fn encode_message(message: &Outgoing) -> Result<String, MailError> {
    let email = super::build_mime(message)?;
    Ok(URL_SAFE.encode(email.formatted()))
}

#[async_trait]
impl Mailer for GmailMailer {
    fn name(&self) -> &'static str {
        "gmail-api"
    }

    fn is_live(&self) -> bool {
        true
    }

    async fn send(&self, message: Outgoing) -> Result<(), MailError> {
        let raw = encode_message(&message)?;
        let token = self.tokens.access_token().await?;

        // `users/me` is the impersonated mailbox, not the service account.
        let resp = self
            .tokens
            .http()
            .post(SEND_URL)
            .bearer_auth(&token)
            .json(&json!({ "raw": raw }))
            .send()
            .await
            .map_err(|e| MailError::Transport(format!("send failed: {e}")))?;

        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }

        let body = resp.text().await.unwrap_or_default();
        let detail = summarise(&body);

        // 5xx and 429 are worth another go. A 403 usually means the From address
        // is not the impersonated mailbox or one of its verified aliases, which
        // no amount of retrying will change.
        if status.is_server_error() || status.as_u16() == 429 {
            Err(MailError::Transport(format!("{status}: {detail}")))
        } else {
            Err(MailError::Rejected(format!(
                "{status}: {detail} (sending as {} on behalf of {})",
                message.from_address,
                self.tokens.impersonate()
            )))
        }
    }
}

/// Load a service account from `GOOGLE_SA_KEY_FILE` or `GOOGLE_SA_KEY_JSON`.
///
/// A file path is preferred: it keeps the key out of the process environment,
/// where it would otherwise show up in `docker inspect` and crash dumps.
pub fn load_service_account() -> Option<Result<ServiceAccount, MailError>> {
    let inline = std::env::var("GOOGLE_SA_KEY_JSON")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let path = std::env::var("GOOGLE_SA_KEY_FILE")
        .ok()
        .filter(|s| !s.trim().is_empty());

    let raw = match (path, inline) {
        (Some(p), _) => match std::fs::read_to_string(&p) {
            Ok(s) => s,
            Err(e) => {
                return Some(Err(MailError::NotConfigured(format!(
                    "could not read GOOGLE_SA_KEY_FILE at {p}: {e}"
                ))))
            }
        },
        (None, Some(j)) => j,
        (None, None) => return None,
    };

    Some(serde_json::from_str::<ServiceAccount>(&raw).map_err(|e| {
        MailError::NotConfigured(format!("service account key is not valid JSON: {e}"))
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::email::templates;

    fn outgoing() -> Outgoing {
        Outgoing {
            to: "guest@example.com".into(),
            from_address: "hello@theblendbarokc.com".into(),
            from_name: "The Blend Bar".into(),
            reply_to: Some("replies@theblendbarokc.com".into()),
            body: templates::magic_link("https://x.test/portal/verify?token=abc", 15),
            attachments: Vec::new(),
        }
    }

    #[test]
    fn encodes_a_message_gmail_will_accept() {
        let encoded = encode_message(&outgoing()).expect("should encode");
        // Must be base64url: no '+' or '/', which Gmail's raw field rejects.
        assert!(!encoded.contains('+'), "standard base64 leaked in");
        assert!(!encoded.contains('/'), "standard base64 leaked in");

        let decoded = String::from_utf8(URL_SAFE.decode(&encoded).unwrap()).unwrap();
        // lettre quotes a display name containing spaces, per RFC 5322.
        assert!(decoded.contains(r#"From: "The Blend Bar" <hello@theblendbarokc.com>"#));
        assert!(decoded.contains("To: guest@example.com"));
        assert!(decoded.contains("Reply-To: replies@theblendbarokc.com"));
        assert!(decoded.contains("multipart/alternative"));
        // Both bodies survive the round trip.
        assert!(decoded.contains("text/plain"));
        assert!(decoded.contains("text/html"));
    }

    #[test]
    fn rejects_an_unusable_recipient() {
        let mut m = outgoing();
        m.to = "not an address".into();
        assert!(matches!(
            encode_message(&m),
            Err(MailError::Rejected(_))
        ));
    }

    #[test]
    fn summarises_google_errors_usefully() {
        let body = r#"{"error":"unauthorized_client","error_description":"Client is unauthorized to retrieve access tokens using this method."}"#;
        let s = summarise(body);
        assert!(s.contains("unauthorized_client"));
        assert!(s.contains("unauthorized to retrieve"));
    }

    #[test]
    fn summarise_falls_back_to_raw_text() {
        assert_eq!(summarise("upstream timeout"), "upstream timeout");
    }

    #[test]
    fn a_malformed_key_fails_at_construction_not_at_send_time() {
        let sa = ServiceAccount {
            client_email: "bot@project.iam.gserviceaccount.com".into(),
            private_key: "-----BEGIN PRIVATE KEY-----\nnot a key\n-----END PRIVATE KEY-----".into(),
            private_key_id: "abc".into(),
        };
        assert!(GmailMailer::new(sa, "hello@theblendbarokc.com".into()).is_err());
    }
}
