//! Building the per-platform payloads.
//!
//! Discord, Slack and Teams each want a different JSON shape for what is
//! conceptually the same message, so the content is assembled once as a
//! [`Message`] and rendered three ways. These are pure functions — no network,
//! no database — which makes them the part of the notification system that can
//! actually be tested without credentials.

use serde_json::{json, Value};

use crate::square::money;

/// Brand gold (`--gold` / #ac854a), the accent used across the product.
const BRAND_HEX: &str = "AC854A";
const BRAND_DECIMAL: u32 = 0xAC_85_4A;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    /// A customer bought from a shared scent link. Nobody was at the till.
    OnlineSale,
    /// A deposit settled. Per the published booking terms, that is the moment an
    /// event is actually booked.
    EventBooked,
}

impl EventKind {
    pub fn wire(&self) -> &'static str {
        match self {
            EventKind::OnlineSale => "sale.online",
            EventKind::EventBooked => "event.booked",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "sale.online" => Some(EventKind::OnlineSale),
            "event.booked" => Some(EventKind::EventBooked),
            _ => None,
        }
    }

    fn headline(&self) -> &'static str {
        match self {
            EventKind::OnlineSale => "New online order",
            EventKind::EventBooked => "Event booked — deposit paid",
        }
    }

    fn call_to_action(&self) -> &'static str {
        match self {
            // Said plainly because it is the whole point: this bottle does not
            // exist yet and somebody has to make it.
            EventKind::OnlineSale => "This blend still needs to be made.",
            EventKind::EventBooked => "Confirm the date and add it to the calendar.",
        }
    }
}

/// One notification, assembled before it is rendered for a platform.
#[derive(Debug, Clone)]
pub struct Message {
    pub kind: EventKind,
    /// What was bought, one line per cart item.
    pub lines: Vec<String>,
    pub total_cents: i64,
    pub currency: String,
    pub customer_name: Option<String>,
    /// Only ever populated when the target opts in.
    pub customer_email: Option<String>,
    /// Short cart reference, for looking the order up in the app.
    pub reference: String,
}

impl Message {
    fn total(&self) -> String {
        money::format_cents(self.total_cents, &self.currency)
    }

    fn who(&self) -> String {
        match (&self.customer_name, &self.customer_email) {
            (Some(n), Some(e)) => format!("{n} · {e}"),
            (Some(n), None) => n.clone(),
            (None, Some(e)) => e.clone(),
            (None, None) => "Customer".to_string(),
        }
    }

    fn items(&self) -> String {
        if self.lines.is_empty() {
            "—".to_string()
        } else {
            self.lines.join("\n")
        }
    }

    /// Single-line summary, used as the notification preview on every platform.
    pub fn summary(&self) -> String {
        format!("{} — {} — {}", self.kind.headline(), self.total(), self.who())
    }
}

/// `{"content": …, "embeds": [...]}` — Discord's incoming-webhook shape.
pub fn discord(m: &Message) -> Value {
    json!({
        "username": "The Blend Bar",
        "embeds": [{
            "title": m.kind.headline(),
            "description": m.kind.call_to_action(),
            "color": BRAND_DECIMAL,
            "fields": [
                { "name": "Items", "value": m.items(), "inline": false },
                { "name": "Total", "value": m.total(), "inline": true },
                { "name": "Customer", "value": m.who(), "inline": true },
                { "name": "Reference", "value": m.reference, "inline": false }
            ],
            "footer": { "text": "The Blend Bar" }
        }]
    })
}

/// `{"text": …, "blocks": [...]}` — Slack incoming webhook. `text` is required
/// even alongside blocks: it is what shows in the notification popup and in
/// clients that cannot render blocks.
pub fn slack(m: &Message) -> Value {
    json!({
        "text": m.summary(),
        "blocks": [
            {
                "type": "header",
                "text": { "type": "plain_text", "text": m.kind.headline(), "emoji": true }
            },
            {
                "type": "section",
                "fields": [
                    { "type": "mrkdwn", "text": format!("*Total*\n{}", m.total()) },
                    { "type": "mrkdwn", "text": format!("*Customer*\n{}", m.who()) }
                ]
            },
            {
                "type": "section",
                "text": { "type": "mrkdwn", "text": format!("*Items*\n{}", m.items()) }
            },
            {
                "type": "context",
                "elements": [
                    { "type": "mrkdwn", "text": format!("{} · `{}`", m.kind.call_to_action(), m.reference) }
                ]
            }
        ]
    })
}

/// Office 365 connector "MessageCard" — what a Teams *Incoming Webhook* accepts.
///
/// Microsoft is retiring O365 connectors in favour of Workflows (Power Automate),
/// which take Adaptive Cards instead. MessageCard still works on existing
/// connector URLs; if a channel is migrated to a Workflow URL this payload will
/// need to change shape.
pub fn teams(m: &Message) -> Value {
    let mut facts = vec![
        json!({ "name": "Items", "value": m.items() }),
        json!({ "name": "Total", "value": m.total() }),
        json!({ "name": "Customer", "value": m.who() }),
        json!({ "name": "Reference", "value": m.reference }),
    ];
    facts.push(json!({ "name": "Next", "value": m.kind.call_to_action() }));

    json!({
        "@type": "MessageCard",
        "@context": "https://schema.org/extensions",
        "summary": m.summary(),
        "themeColor": BRAND_HEX,
        "title": m.kind.headline(),
        "sections": [{ "facts": facts, "markdown": false }]
    })
}

/// Render for the named platform. Unknown platforms are a configuration bug, so
/// this returns `None` rather than guessing a shape.
pub fn render(platform: &str, m: &Message) -> Option<Value> {
    match platform {
        "discord" => Some(discord(m)),
        "slack" => Some(slack(m)),
        "teams" => Some(teams(m)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg() -> Message {
        Message {
            kind: EventKind::OnlineSale,
            lines: vec!["Golden Hour (3.4 oz)".into()],
            total_cents: 6000,
            currency: "USD".into(),
            customer_name: Some("Alex".into()),
            customer_email: None,
            reference: "cart 1a2b3c4d".into(),
        }
    }

    #[test]
    fn every_platform_renders_and_carries_the_money() {
        for platform in ["discord", "slack", "teams"] {
            let v = render(platform, &msg()).expect("should render");
            let s = serde_json::to_string(&v).unwrap();
            assert!(s.contains("$60.00"), "{platform} lost the total: {s}");
            assert!(s.contains("Golden Hour"), "{platform} lost the item: {s}");
        }
    }

    #[test]
    fn unknown_platform_renders_nothing() {
        // Better to drop the delivery and log than to POST a guessed shape.
        assert!(render("mattermost", &msg()).is_none());
    }

    #[test]
    fn email_is_absent_unless_supplied() {
        // The opt-in lives in the caller; this proves the formatter never invents
        // an email or leaks one through another field.
        let s = serde_json::to_string(&render("slack", &msg()).unwrap()).unwrap();
        assert!(!s.contains('@'), "email leaked into payload: {s}");

        let mut with = msg();
        with.customer_email = Some("alex@example.com".into());
        let s = serde_json::to_string(&render("slack", &with).unwrap()).unwrap();
        assert!(s.contains("alex@example.com"));
    }

    #[test]
    fn slack_always_sets_top_level_text() {
        // Without it Slack shows an empty notification popup.
        let v = slack(&msg());
        assert!(v.get("text").and_then(|t| t.as_str()).is_some_and(|s| !s.is_empty()));
    }

    #[test]
    fn teams_uses_the_messagecard_envelope() {
        let v = teams(&msg());
        assert_eq!(v["@type"], "MessageCard");
        assert_eq!(v["themeColor"], BRAND_HEX);
    }

    #[test]
    fn event_booked_reads_differently_from_a_sale() {
        let mut e = msg();
        e.kind = EventKind::EventBooked;
        e.lines = vec!["Event deposit (50%)".into()];
        let sale = serde_json::to_string(&discord(&msg())).unwrap();
        let booked = serde_json::to_string(&discord(&e)).unwrap();
        assert!(booked.contains("Event booked"));
        assert!(!sale.contains("Event booked"));
        // The two call-to-actions must not be interchangeable — the whole value
        // of the notification is knowing which thing just happened.
        assert!(booked.contains("calendar"));
        assert!(sale.contains("needs to be made"));
    }

    #[test]
    fn handles_a_customer_with_no_name_or_email() {
        let mut m = msg();
        m.customer_name = None;
        let s = serde_json::to_string(&render("discord", &m).unwrap()).unwrap();
        assert!(s.contains("Customer"));
    }

    #[test]
    fn multiple_lines_are_all_present() {
        let mut m = msg();
        m.lines = vec!["Golden Hour (3.4 oz)".into(), "Event deposit (50%)".into()];
        m.total_cents = 9998;
        let s = serde_json::to_string(&render("teams", &m).unwrap()).unwrap();
        assert!(s.contains("Golden Hour"));
        assert!(s.contains("Event deposit"));
        assert!(s.contains("$99.98"));
    }

    #[test]
    fn wire_names_round_trip() {
        for k in [EventKind::OnlineSale, EventKind::EventBooked] {
            assert_eq!(EventKind::from_wire(k.wire()), Some(k));
        }
        assert_eq!(EventKind::from_wire("sale.instore"), None);
    }
}
