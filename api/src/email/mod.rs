//! Outbound email through the Google Workspace SMTP relay.
//!
//! ## Why the relay, and not the alternatives
//!
//! Google has spent several years closing the easy doors. Basic username/password
//! SMTP auth is gone; app passwords still work but Google's own documentation
//! calls them "not recommended" — they cannot be scoped, and anyone holding the
//! 16 characters has full send rights on that mailbox. The Gmail API with a
//! service account and domain-wide delegation is the other supported route, but
//! it means a private key on disk, token refresh, multi-party admin approval for
//! the delegation, and a security assessment if restricted scopes are involved.
//!
//! `smtp-relay.gmail.com` is Google's documented path for exactly this shape —
//! an application on a server sending as a Workspace domain. Authorised by
//! **IP allowlist**, which this deployment can satisfy because it is one VPS with
//! a fixed address, it needs *no credentials on the box at all*. SMTP AUTH can be
//! layered on as a second factor; both are supported here.
//!
//! ## The boundary
//!
//! Everything goes through the [`Mailer`] trait, so the app runs on an in-process
//! mock until a relay is configured — the same arrangement as the Square
//! integration, and for the same reason: the interesting logic stays testable on
//! a box with no credentials.

pub mod credentials;
pub mod dispatch;
pub mod gmail;
pub mod templates;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::{header::ContentType, Attachment, MultiPart, SinglePart};
use lettre::transport::smtp::authentication::Credentials;
use lettre::transport::smtp::client::{Tls, TlsParameters};
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

use templates::Rendered;

/// One addressed message.
pub struct Outgoing {
    pub to: String,
    pub from_address: String,
    pub from_name: String,
    pub reply_to: Option<String>,
    pub body: Rendered,
    /// Usually empty. Scheduled backups are the only sender that attaches
    /// anything, and what they attach is an encrypted database dump.
    pub attachments: Vec<Attached>,
}

/// A file to hang off a message.
pub struct Attached {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// Build the MIME message that both transports send.
///
/// Shared rather than written twice: the SMTP relay and the Gmail API held
/// identical copies of this, which is how a change lands in one path and not the
/// other — and "attachments work over SMTP but not over Gmail" is exactly the
/// class of bug nobody notices until the backup they need arrives empty.
///
/// A message with nothing attached is `multipart/alternative`, byte-for-byte as
/// before. With attachments it becomes `multipart/mixed` wrapping that
/// alternative group: some clients will not display an attachment that is a
/// sibling of the text parts rather than of the group.
pub fn build_mime(message: &Outgoing) -> Result<Message, MailError> {
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

    // Text first, then HTML. Order matters — clients display the last part they
    // can render.
    let alternative = MultiPart::alternative()
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(message.body.text.clone()),
        )
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_HTML)
                .body(message.body.html.clone()),
        );

    if message.attachments.is_empty() {
        return builder
            .multipart(alternative)
            .map_err(|e| MailError::Rejected(format!("could not build message: {e}")));
    }

    let mut mixed = MultiPart::mixed().multipart(alternative);
    for file in &message.attachments {
        let content_type = ContentType::parse(&file.content_type).map_err(|e| {
            MailError::Rejected(format!("bad content type '{}': {e}", file.content_type))
        })?;
        mixed = mixed.singlepart(
            Attachment::new(file.filename.clone()).body(file.bytes.clone(), content_type),
        );
    }

    builder
        .multipart(mixed)
        .map_err(|e| MailError::Rejected(format!("could not build message: {e}")))
}

#[derive(Debug)]
pub enum MailError {
    /// Nothing is configured, or the configuration is incomplete.
    NotConfigured(String),
    /// The address or message was rejected outright; retrying will not help.
    Rejected(String),
    /// Connection or transient server problem — worth another attempt.
    Transport(String),
}

impl MailError {
    pub fn retryable(&self) -> bool {
        matches!(self, MailError::Transport(_))
    }
}

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailError::NotConfigured(m) => write!(f, "email is not configured: {m}"),
            MailError::Rejected(m) => write!(f, "rejected: {m}"),
            MailError::Transport(m) => write!(f, "transport error: {m}"),
        }
    }
}

#[async_trait]
pub trait Mailer: Send + Sync {
    /// Short label for logs and the admin panel.
    fn name(&self) -> &'static str;
    /// True when mail actually leaves the building.
    fn is_live(&self) -> bool;
    async fn send(&self, message: Outgoing) -> Result<(), MailError>;
}

// --- SMTP -------------------------------------------------------------------

pub struct SmtpMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    host: String,
}

impl SmtpMailer {
    pub fn new(
        host: String,
        port: u16,
        username: Option<String>,
        password: Option<String>,
    ) -> Result<Self, MailError> {
        // STARTTLS on 587 is what the relay documents. Built explicitly rather
        // than via the `relay()` helper so the port is honoured and the
        // connection is refused if TLS cannot be negotiated — an unencrypted
        // fallback would put sign-in links on the wire in clear text.
        let tls = TlsParameters::new(host.clone())
            .map_err(|e| MailError::NotConfigured(format!("TLS setup failed: {e}")))?;

        let mut builder = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host.as_str())
            .port(port)
            .tls(Tls::Required(tls))
            .timeout(Some(Duration::from_secs(20)));

        // Optional: the relay can authorise by IP alone, in which case there are
        // no credentials to hold.
        if let (Some(u), Some(p)) = (username, password) {
            builder = builder.credentials(Credentials::new(u, p));
        }

        Ok(Self {
            transport: builder.build(),
            host,
        })
    }
}

#[async_trait]
impl Mailer for SmtpMailer {
    fn name(&self) -> &'static str {
        "smtp"
    }

    fn is_live(&self) -> bool {
        true
    }

    async fn send(&self, message: Outgoing) -> Result<(), MailError> {
        let email = build_mime(&message)?;

        self.transport.send(email).await.map_err(|e| {
            let text = e.to_string();
            // A permanent SMTP reply (5xx) means the relay will say the same
            // thing next time — usually an unauthorised sender or a bad
            // recipient. Retrying those just repeats the rejection.
            if e.is_permanent() {
                MailError::Rejected(format!("{} said: {text}", self.host))
            } else {
                MailError::Transport(text)
            }
        })?;
        Ok(())
    }
}

// --- Mock -------------------------------------------------------------------

/// Used whenever no relay is configured. Logs what *would* have been sent so the
/// magic-link flow stays usable in development, and reports itself as not live so
/// the admin panel can say so plainly.
#[derive(Default)]
pub struct MockMailer;

#[async_trait]
impl Mailer for MockMailer {
    fn name(&self) -> &'static str {
        "mock"
    }

    fn is_live(&self) -> bool {
        false
    }

    async fn send(&self, message: Outgoing) -> Result<(), MailError> {
        tracing::warn!(
            to = %message.to,
            subject = %message.body.subject,
            "[mock mailer] NOT SENT — no SMTP relay configured. Body follows:\n{}",
            message.body.text
        );
        Ok(())
    }
}

// --- Wiring -----------------------------------------------------------------

/// Build the mailer from the environment.
///
/// Order of preference:
///
/// 1. **Gmail API** with a service account, when a key and a mailbox to
///    impersonate are both present. Preferred because it does not depend on the
///    server's IP address, sends as a real mailbox (so the message appears in
///    that account's Sent folder), and has nothing that expires.
/// 2. **SMTP relay**, when `SMTP_HOST` is set.
/// 3. **Mock**, which logs instead of sending.
///
/// A half-configured backend is never silently downgraded to the next one
/// without saying so — a misconfiguration that quietly falls back to "logs
/// nothing to anyone" is how a business discovers its email is off by hearing
/// about it from a customer.
pub fn from_env() -> Arc<dyn Mailer> {
    build(None)
}

/// Rebuild the mailer, optionally with the mailbox an admin configured in the
/// database. Called at boot and again whenever credentials change, so saving them
/// takes effect without a restart.
pub fn build(stored_impersonate: Option<String>) -> Arc<dyn Mailer> {
    // A key from the environment wins over one uploaded through the browser: an
    // ops-managed deployment should not be silently overridden by a form, and if
    // both exist the environment is the one an operator can actually see.
    let loaded_key = gmail::load_service_account().or_else(credentials::load_stored);

    if let Some(loaded) = loaded_key {
        let impersonate = std::env::var("GOOGLE_IMPERSONATE")
            .ok()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or(stored_impersonate);

        match (loaded, impersonate) {
            (Ok(account), Some(mailbox)) => {
                let client_email = account.client_email.clone();
                match gmail::GmailMailer::new(account, mailbox.clone()) {
                    Ok(mailer) => {
                        tracing::info!(
                            service_account = %client_email,
                            sending_as = %mailbox,
                            "email: Gmail API via service account (domain-wide delegation)"
                        );
                        return Arc::new(mailer);
                    }
                    Err(e) => tracing::error!("email: Gmail service account unusable ({e})"),
                }
            }
            (Ok(_), None) => tracing::error!(
                "email: a Google service-account key is present but GOOGLE_IMPERSONATE \
                 is unset. Set it to the Workspace mailbox the app should send as — \
                 the key alone cannot send, because a service account has no inbox."
            ),
            (Err(e), _) => tracing::error!("email: {e}"),
        }
    }

    let host = std::env::var("SMTP_HOST")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    let Some(host) = host else {
        tracing::warn!(
            "No email transport configured — using the mock mailer. Set either \
             GOOGLE_SA_KEY_FILE + GOOGLE_IMPERSONATE (Gmail API, preferred) or \
             SMTP_HOST (Workspace relay). Until then sign-in links are written to \
             this log instead of emailed, and no customer receives anything."
        );
        return Arc::new(MockMailer);
    };

    let port: u16 = std::env::var("SMTP_PORT")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(587);

    let username = std::env::var("SMTP_USERNAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let password = std::env::var("SMTP_PASSWORD")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    if username.is_some() != password.is_some() {
        tracing::error!(
            "SMTP_USERNAME and SMTP_PASSWORD must be set together — falling back to \
             the mock mailer rather than attempting a half-configured login"
        );
        return Arc::new(MockMailer);
    }

    match SmtpMailer::new(host.clone(), port, username.clone(), password) {
        Ok(mailer) => {
            tracing::info!(
                %host, port,
                auth = if username.is_some() { "smtp-auth" } else { "ip-allowlist" },
                "email: live SMTP relay"
            );
            Arc::new(mailer)
        }
        Err(e) => {
            tracing::error!("email: could not build SMTP transport ({e}) — using the mock");
            Arc::new(MockMailer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn the_mock_never_fails_and_is_not_live() {
        let m = MockMailer;
        assert!(!m.is_live());
        let r = m
            .send(Outgoing {
                to: "someone@example.com".into(),
                from_address: "hello@theblendbarokc.com".into(),
                from_name: "The Blend Bar".into(),
                reply_to: None,
                body: templates::test_message("https://x.test"),
                attachments: Vec::new(),
            })
            .await;
        assert!(r.is_ok());
    }

    #[test]
    fn only_transport_failures_are_retried() {
        // Retrying a 5xx just repeats the rejection; retrying a dropped
        // connection is the whole point of a queue.
        assert!(MailError::Transport("timeout".into()).retryable());
        assert!(!MailError::Rejected("550 not authorised".into()).retryable());
        assert!(!MailError::NotConfigured("no host".into()).retryable());
    }

    #[test]
    fn smtp_transport_builds_for_the_workspace_relay() {
        assert!(SmtpMailer::new("smtp-relay.gmail.com".into(), 587, None, None).is_ok());
        assert!(SmtpMailer::new(
            "smtp-relay.gmail.com".into(),
            587,
            Some("u@d.com".into()),
            Some("p".into())
        )
        .is_ok());
    }
}
