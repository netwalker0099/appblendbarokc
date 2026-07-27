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

use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::engine::general_purpose::URL_SAFE;
use base64::Engine;
use jsonwebtoken::{Algorithm, EncodingKey, Header};
use lettre::message::{header::ContentType, MultiPart, SinglePart};
use lettre::Message;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

use super::{MailError, Mailer, Outgoing};

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
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

#[derive(Serialize)]
struct Assertion<'a> {
    iss: &'a str,
    scope: &'a str,
    aud: &'a str,
    /// The mailbox to act as. This claim *is* the domain-wide delegation.
    sub: &'a str,
    iat: u64,
    exp: u64,
}

struct CachedToken {
    value: String,
    /// When it stops being usable. Refreshed early, never on the boundary.
    expires_at: Instant,
}

pub struct GmailMailer {
    client: reqwest::Client,
    account: ServiceAccount,
    key: EncodingKey,
    impersonate: String,
    token: Mutex<Option<CachedToken>>,
}

impl GmailMailer {
    pub fn new(account: ServiceAccount, impersonate: String) -> Result<Self, MailError> {
        // Google issues PKCS#8 PEM. Parsed once at startup so a malformed key is
        // a boot-time error rather than a surprise on the first customer email.
        let key = EncodingKey::from_rsa_pem(account.private_key.as_bytes()).map_err(|e| {
            MailError::NotConfigured(format!(
                "service account private_key is not a usable RSA PEM: {e}"
            ))
        })?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| MailError::NotConfigured(e.to_string()))?;

        Ok(Self {
            client,
            account,
            key,
            impersonate,
            token: Mutex::new(None),
        })
    }

    /// A valid access token, minted on demand and cached until shortly before it
    /// expires.
    async fn access_token(&self) -> Result<String, MailError> {
        let mut guard = self.token.lock().await;

        if let Some(cached) = guard.as_ref() {
            if Instant::now() < cached.expires_at {
                return Ok(cached.value.clone());
            }
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| MailError::Transport(e.to_string()))?
            .as_secs();

        let claims = Assertion {
            iss: &self.account.client_email,
            scope: SCOPE,
            aud: TOKEN_URL,
            sub: &self.impersonate,
            iat: now,
            // Google caps assertion lifetime at an hour.
            exp: now + 3600,
        };

        let mut header = Header::new(Algorithm::RS256);
        if !self.account.private_key_id.is_empty() {
            header.kid = Some(self.account.private_key_id.clone());
        }

        let assertion = jsonwebtoken::encode(&header, &claims, &self.key)
            .map_err(|e| MailError::NotConfigured(format!("could not sign assertion: {e}")))?;

        let resp = self
            .client
            .post(TOKEN_URL)
            .form(&[
                ("grant_type", "urn:ietf:params:oauth:grant-type:jwt-bearer"),
                ("assertion", &assertion),
            ])
            .send()
            .await
            .map_err(|e| MailError::Transport(format!("token request failed: {e}")))?;

        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();

        if !status.is_success() {
            // The usual cause is the delegation not being authorised, or being
            // authorised for a different scope. Say so — the raw Google error is
            // terse and this is the step people get wrong.
            return Err(MailError::NotConfigured(format!(
                "Google refused the service-account assertion ({status}): {}. \
                 Check that client id {} is authorised for {SCOPE} under Admin → \
                 Security → API controls → Domain-wide delegation, and that {} is \
                 a real mailbox on the domain.",
                summarise(&text),
                self.account.client_email,
                self.impersonate,
            )));
        }

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            expires_in: u64,
        }

        let parsed: TokenResponse = serde_json::from_str(&text)
            .map_err(|e| MailError::Transport(format!("unreadable token response: {e}")))?;

        // Refresh a minute early so a request never races the expiry.
        let expires_at =
            Instant::now() + Duration::from_secs(parsed.expires_in.saturating_sub(60).max(30));

        *guard = Some(CachedToken {
            value: parsed.access_token.clone(),
            expires_at,
        });

        Ok(parsed.access_token)
    }
}

/// Google's errors arrive as `{"error":"...","error_description":"..."}`.
fn summarise(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.chars().take(200).collect();
    };
    let code = v.get("error").and_then(|x| x.as_str()).unwrap_or("");
    let detail = v
        .get("error_description")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if code.is_empty() && detail.is_empty() {
        body.chars().take(200).collect()
    } else {
        format!("{code}: {detail}").trim_matches(|c| c == ':' || c == ' ').to_string()
    }
}

/// Build the RFC 5322 message and base64url it, which is what `raw` wants.
fn encode_message(message: &Outgoing) -> Result<String, MailError> {
    let from = format!("{} <{}>", message.from_name, message.from_address)
        .parse()
        .map_err(|e| MailError::NotConfigured(format!("bad from address: {e}")))?;
    let to = message
        .to
        .parse()
        .map_err(|e| MailError::Rejected(format!("bad recipient '{}': {e}", message.to)))?;

    let mut builder = Message::builder()
        .from(from)
        .to(to)
        .subject(message.body.subject.clone());

    if let Some(reply_to) = &message.reply_to {
        if let Ok(parsed) = reply_to.parse() {
            builder = builder.reply_to(parsed);
        }
    }

    let email = builder
        .multipart(
            MultiPart::alternative()
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_PLAIN)
                        .body(message.body.text.clone()),
                )
                .singlepart(
                    SinglePart::builder()
                        .header(ContentType::TEXT_HTML)
                        .body(message.body.html.clone()),
                ),
        )
        .map_err(|e| MailError::Rejected(format!("could not build message: {e}")))?;

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
        let token = self.access_token().await?;

        // `users/me` is the impersonated mailbox, not the service account.
        let resp = self
            .client
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
                message.from_address, self.impersonate
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
