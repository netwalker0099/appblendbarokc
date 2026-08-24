//! Minting Google access tokens from a service account with domain-wide
//! delegation.
//!
//! This was inside the Gmail mailer, which was fine while email was the only
//! thing that talked to Google. Scheduled backups upload to Drive and need the
//! same signed-assertion dance against a different scope, so it lives here and
//! both callers share it. The alternative — a second copy of the JWT logic —
//! drifts: the copy that gets the timing fix is never the copy that breaks at
//! 3am.
//!
//! See `email::gmail` for why a service account is used here rather than
//! three-legged OAuth. The short version: a user-consent refresh token can lapse,
//! and when it does, whatever depended on it stops working until a human signs
//! in again. That is a bad property for sign-in emails and a worse one for
//! backups, which nobody is watching.

use std::time::{Duration, Instant};

use jsonwebtoken::{Algorithm, EncodingKey, Header};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

use crate::email::gmail::ServiceAccount;
use crate::email::MailError;

const TOKEN_URL: &str = "https://oauth2.googleapis.com/token";

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

/// A cached access token for one (service account, user, scope) triple.
pub struct TokenSource {
    client: reqwest::Client,
    account: ServiceAccount,
    key: EncodingKey,
    impersonate: String,
    scope: &'static str,
    token: Mutex<Option<CachedToken>>,
}

impl TokenSource {
    pub fn new(
        account: ServiceAccount,
        impersonate: String,
        scope: &'static str,
    ) -> Result<Self, MailError> {
        // Google issues PKCS#8 PEM. Parsed once up front so a malformed key is a
        // configuration error caught at startup rather than a surprise on the
        // first real send.
        let key = EncodingKey::from_rsa_pem(account.private_key.as_bytes()).map_err(|e| {
            MailError::NotConfigured(format!(
                "service account private_key is not a usable RSA PEM: {e}"
            ))
        })?;

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| MailError::NotConfigured(e.to_string()))?;

        Ok(Self {
            client,
            account,
            key,
            impersonate,
            scope,
            token: Mutex::new(None),
        })
    }

    pub fn impersonate(&self) -> &str {
        &self.impersonate
    }

    /// A shared HTTP client, so callers reuse connections rather than building
    /// a new pool per request.
    pub fn http(&self) -> &reqwest::Client {
        &self.client
    }

    /// A valid access token, minted on demand and cached until shortly before it
    /// expires.
    pub async fn access_token(&self) -> Result<String, MailError> {
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
            scope: self.scope,
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
            // authorised for a different scope — and a scope added later (Drive,
            // after Gmail) is the common version of that. The raw Google error is
            // terse, so name the exact thing to go and check.
            return Err(MailError::NotConfigured(format!(
                "Google refused the service-account assertion ({status}): {}. \
                 Check that client id {} is authorised for {} under Admin → \
                 Security → API controls → Domain-wide delegation, and that {} is \
                 a real mailbox on the domain.",
                summarise(&text),
                self.account.client_email,
                self.scope,
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
pub fn summarise(body: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(body) else {
        return body.chars().take(200).collect();
    };
    // The Drive API nests it one deeper than the token endpoint does.
    if let Some(message) = v
        .get("error")
        .and_then(|e| e.get("message"))
        .and_then(|m| m.as_str())
    {
        return message.chars().take(300).collect();
    }
    let code = v.get("error").and_then(|x| x.as_str()).unwrap_or("");
    let detail = v
        .get("error_description")
        .and_then(|x| x.as_str())
        .unwrap_or("");
    if code.is_empty() && detail.is_empty() {
        body.chars().take(200).collect()
    } else {
        format!("{code}: {detail}")
            .trim_matches(|c| c == ':' || c == ' ')
            .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn google_errors_are_summarised_in_both_shapes() {
        // Token endpoint.
        assert_eq!(
            summarise(r#"{"error":"unauthorized_client","error_description":"Client is unauthorized"}"#),
            "unauthorized_client: Client is unauthorized"
        );
        // Drive API.
        assert_eq!(
            summarise(r#"{"error":{"code":403,"message":"Insufficient permission"}}"#),
            "Insufficient permission"
        );
        // Anything else comes back as-is rather than as an empty string, so a
        // surprising failure is still diagnosable from the log.
        assert_eq!(summarise("<html>502</html>"), "<html>502</html>");
    }
}
