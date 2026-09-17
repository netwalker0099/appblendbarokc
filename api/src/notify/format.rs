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
    /// Someone asked about an event from the public booking form. No money has
    /// moved and nothing is committed — this is the first contact.
    EventEnquiry,
}

impl EventKind {
    pub fn wire(&self) -> &'static str {
        match self {
            EventKind::OnlineSale => "sale.online",
            EventKind::EventBooked => "event.booked",
            EventKind::EventEnquiry => "event.enquiry",
        }
    }

    pub fn from_wire(s: &str) -> Option<Self> {
        match s {
            "sale.online" => Some(EventKind::OnlineSale),
            "event.booked" => Some(EventKind::EventBooked),
            "event.enquiry" => Some(EventKind::EventEnquiry),
            _ => None,
        }
    }

    /// The per-target opt-in column that gates this event. Kept beside the wire
    /// name so adding an event forces you to answer "what turns it off?".
    pub fn target_column(&self) -> &'static str {
        match self {
            EventKind::OnlineSale => "notify_online_sale",
            EventKind::EventBooked => "notify_event_booked",
            EventKind::EventEnquiry => "notify_event_enquiry",
        }
    }

    fn headline(&self) -> &'static str {
        match self {
            EventKind::OnlineSale => "New online order",
            EventKind::EventBooked => "Event booked — deposit paid",
            EventKind::EventEnquiry => "New event enquiry",
        }
    }

    /// What the body of the message actually is. A cart has items; an enquiry
    /// has somebody's message, and calling that "Items" reads as a bug.
    fn body_label(&self) -> &'static str {
        match self {
            EventKind::OnlineSale | EventKind::EventBooked => "Items",
            EventKind::EventEnquiry => "Message",
        }
    }

    fn call_to_action(&self) -> &'static str {
        match self {
            // Said plainly because it is the whole point: this bottle does not
            // exist yet and somebody has to make it.
            EventKind::OnlineSale => "This blend still needs to be made.",
            EventKind::EventBooked => "Confirm the date and add it to the calendar.",
            // Nothing is booked yet and the person is waiting on a human. The
            // contact details are in the app, not necessarily in this message.
            EventKind::EventEnquiry => "Open Admin → Enquiries to reply.",
        }
    }
}

/// One notification, assembled before it is rendered for a platform.
#[derive(Debug, Clone)]
pub struct Message {
    pub kind: EventKind,
    /// What was bought, one line per cart item. For an enquiry, what they wrote.
    pub lines: Vec<String>,
    /// `None` for events where no money is involved. An enquiry showing "$0.00"
    /// would read as a free order rather than as a question.
    pub total_cents: Option<i64>,
    pub currency: String,
    pub customer_name: Option<String>,
    /// Only ever populated when the target opts in.
    pub customer_email: Option<String>,
    /// Short reference, for looking the subject up in the app.
    pub reference: String,
    /// Extra labelled facts, rendered in order after the items. Lets an enquiry
    /// carry its event date without pretending to be a cart line.
    pub facts: Vec<(String, String)>,
}

impl Message {
    fn total(&self) -> Option<String> {
        self.total_cents
            .map(|c| money::format_cents(c, &self.currency))
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
        match self.total() {
            Some(total) => format!("{} — {} — {}", self.kind.headline(), total, self.who()),
            None => format!("{} — {}", self.kind.headline(), self.who()),
        }
    }
}

/// `{"content": …, "embeds": [...]}` — Discord's incoming-webhook shape.
pub fn discord(m: &Message) -> Value {
    let mut fields = vec![json!({ "name": m.kind.body_label(), "value": m.items(), "inline": false })];
    if let Some(total) = m.total() {
        fields.push(json!({ "name": "Total", "value": total, "inline": true }));
    }
    fields.push(json!({ "name": "Customer", "value": m.who(), "inline": true }));
    for (name, value) in &m.facts {
        fields.push(json!({ "name": name, "value": value, "inline": true }));
    }
    fields.push(json!({ "name": "Reference", "value": m.reference, "inline": false }));

    json!({
        "username": "The Blend Bar",
        "embeds": [{
            "title": m.kind.headline(),
            "description": m.kind.call_to_action(),
            "color": BRAND_DECIMAL,
            "fields": fields,
            "footer": { "text": "The Blend Bar" }
        }]
    })
}

/// Slack renders these two-up. Total is omitted entirely when there is no money
/// in the event, rather than shown as zero.
fn summary_fields(m: &Message) -> Vec<Value> {
    let mut fields = Vec::new();
    if let Some(total) = m.total() {
        fields.push(json!({ "type": "mrkdwn", "text": format!("*Total*\n{total}") }));
    }
    fields.push(json!({ "type": "mrkdwn", "text": format!("*Customer*\n{}", m.who()) }));
    for (name, value) in &m.facts {
        fields.push(json!({ "type": "mrkdwn", "text": format!("*{name}*\n{value}") }));
    }
    fields
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
                "fields": summary_fields(m)
            },
            {
                "type": "section",
                "text": { "type": "mrkdwn", "text": format!("*{}*\n{}", m.kind.body_label(), m.items()) }
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
    let mut facts = vec![json!({ "name": m.kind.body_label(), "value": m.items() })];
    if let Some(total) = m.total() {
        facts.push(json!({ "name": "Total", "value": total }));
    }
    facts.push(json!({ "name": "Customer", "value": m.who() }));
    for (name, value) in &m.facts {
        facts.push(json!({ "name": name, "value": value }));
    }
    facts.push(json!({ "name": "Reference", "value": m.reference }));
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
            total_cents: Some(6000),
            currency: "USD".into(),
            customer_name: Some("Alex".into()),
            customer_email: None,
            reference: "cart 1a2b3c4d".into(),
            facts: vec![],
        }
    }

    fn enquiry() -> Message {
        Message {
            kind: EventKind::EventEnquiry,
            lines: vec!["Bridal shower, 20 guests, downtown OKC.".into()],
            total_cents: None,
            currency: "USD".into(),
            customer_name: Some("Ada Lovelace".into()),
            customer_email: None,
            reference: "enquiry 1a2b3c4d".into(),
            facts: vec![("Event date".into(), "2027-03-14".into())],
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
        m.total_cents = Some(9998);
        let s = serde_json::to_string(&render("teams", &m).unwrap()).unwrap();
        assert!(s.contains("Golden Hour"));
        assert!(s.contains("Event deposit"));
        assert!(s.contains("$99.98"));
    }

    #[test]
    fn an_enquiry_never_shows_a_money_total() {
        // "$0.00" on an enquiry reads as a free order. The field must be gone,
        // not zero — on every platform.
        for platform in ["discord", "slack", "teams"] {
            let s = serde_json::to_string(&render(platform, &enquiry()).unwrap()).unwrap();
            assert!(!s.contains("$0.00"), "{platform} rendered a zero total: {s}");
            assert!(!s.contains("Total"), "{platform} kept the Total field: {s}");
            assert!(s.contains("2027-03-14"), "{platform} lost the event date: {s}");
            assert!(s.contains("Bridal shower"), "{platform} lost the details: {s}");
        }
    }

    #[test]
    fn an_enquiry_body_is_not_labelled_items() {
        // It is the customer's message, not a cart.
        for platform in ["discord", "slack", "teams"] {
            let s = serde_json::to_string(&render(platform, &enquiry()).unwrap()).unwrap();
            assert!(!s.contains("Items"), "{platform} called the message Items: {s}");
            assert!(s.contains("Message"), "{platform} lost the label: {s}");
        }
        // A real order keeps the old wording.
        let s = serde_json::to_string(&render("discord", &msg()).unwrap()).unwrap();
        assert!(s.contains("Items"));
    }

    #[test]
    fn an_enquiry_summary_still_says_who_and_what() {
        let s = enquiry().summary();
        assert!(s.contains("New event enquiry"), "{s}");
        assert!(s.contains("Ada Lovelace"), "{s}");
        // No stray separator where the money used to be.
        assert!(!s.contains("—  —"), "{s}");
    }

    #[test]
    fn an_enquiry_does_not_read_like_a_booking() {
        // These two are one step apart in the real world and the whole value of
        // the notification is knowing which one just happened.
        let enq = serde_json::to_string(&discord(&enquiry())).unwrap();
        let mut booked = enquiry();
        booked.kind = EventKind::EventBooked;
        booked.total_cents = Some(25000);
        let booked = serde_json::to_string(&discord(&booked)).unwrap();
        assert!(enq.contains("New event enquiry"));
        assert!(!enq.contains("Event booked"));
        assert!(booked.contains("Event booked"));
    }

    #[test]
    fn enquiry_email_still_respects_the_opt_in() {
        // Same rule as an order: a chat channel is a third party.
        let s = serde_json::to_string(&render("slack", &enquiry()).unwrap()).unwrap();
        assert!(!s.contains('@'), "email leaked: {s}");
        let mut with = enquiry();
        with.customer_email = Some("ada@example.com".into());
        let s = serde_json::to_string(&render("slack", &with).unwrap()).unwrap();
        assert!(s.contains("ada@example.com"));
    }

    #[test]
    fn wire_names_round_trip() {
        for k in [EventKind::OnlineSale, EventKind::EventBooked, EventKind::EventEnquiry] {
            assert_eq!(EventKind::from_wire(k.wire()), Some(k));
        }
        assert_eq!(EventKind::from_wire("sale.instore"), None);
    }
}
