//! Message bodies.
//!
//! Every message is built as plain text *and* HTML. The plain part is not a
//! formality: a sign-in link that only renders in an HTML client is a sign-in
//! link that fails for anyone whose mail app blocks HTML, and it materially
//! improves the odds of landing in an inbox rather than spam.
//!
//! Pure functions, so this is the part of the email system that can be tested
//! properly without a mail server.

/// Everything needed to send one message.
pub struct Rendered {
    pub subject: String,
    pub text: String,
    pub html: String,
}

/// Escape for interpolation into HTML. Customer names and blend names are
/// user-supplied and land in the markup.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// Shared shell. Inline styles only — mail clients strip <style> blocks, and
/// Gmail in particular discards anything in <head>.
fn wrap(heading: &str, body_html: &str) -> String {
    format!(
        r#"<div style="margin:0;padding:24px;background:#fbf7ef;font-family:Helvetica,Arial,sans-serif;color:#4c4235;">
  <div style="max-width:520px;margin:0 auto;background:#ffffff;border:1px solid #e2d7c1;border-radius:3px;padding:32px;">
    <p style="margin:0 0 4px;font-size:11px;letter-spacing:0.28em;text-transform:uppercase;color:#8c6a36;font-weight:600;">The Blend Bar</p>
    <h1 style="margin:0 0 16px;font-family:Georgia,'Times New Roman',serif;font-weight:400;font-size:24px;color:#211c15;">{heading}</h1>
    {body_html}
  </div>
  <p style="max-width:520px;margin:16px auto 0;font-size:12px;color:#897a63;text-align:center;">
    The Blend Bar &middot; Oklahoma City
  </p>
</div>"#
    )
}

fn button(href: &str, label: &str) -> String {
    format!(
        r#"<p style="margin:24px 0;"><a href="{href}" style="display:inline-block;background:#ac854a;color:#ffffff;text-decoration:none;padding:14px 28px;border-radius:2px;font-size:13px;font-weight:600;letter-spacing:0.12em;text-transform:uppercase;">{label}</a></p>"#
    )
}

/// Portal sign-in link.
pub fn magic_link(link: &str, minutes: i64) -> Rendered {
    let text = format!(
        "Sign in to The Blend Bar\n\n\
         Open this link to see your saved blends and reorder:\n\n{link}\n\n\
         The link works once and expires in {minutes} minutes.\n\n\
         If you didn't ask to sign in, you can ignore this email — nobody can \
         get into your account without this link.\n\n\
         The Blend Bar, Oklahoma City"
    );

    // The URL is echoed as text under the button: some clients strip the anchor,
    // and a sign-in mail that cannot be actioned is worse than useless.
    let html = wrap(
        "Sign in",
        &format!(
            r#"<p style="margin:0 0 8px;font-size:15px;line-height:1.6;">Open the link below to see your saved blends and reorder.</p>
    {}
    <p style="margin:0 0 8px;font-size:13px;color:#897a63;line-height:1.6;">Or paste this into your browser:<br><span style="word-break:break-all;color:#8c6a36;">{}</span></p>
    <p style="margin:16px 0 0;font-size:13px;color:#897a63;line-height:1.6;">The link works once and expires in {} minutes. If you didn&rsquo;t ask to sign in, you can ignore this email.</p>"#,
            button(link, "Sign in"),
            esc(link),
            minutes
        ),
    );

    Rendered {
        subject: "Your Blend Bar sign-in link".to_string(),
        text,
        html,
    }
}

/// "Your blend is ready" — the promise made on the online-order thank-you page.
pub fn order_ready(customer_name: Option<&str>, what: &str, portal_url: &str) -> Rendered {
    let greeting = match customer_name {
        Some(n) if !n.trim().is_empty() => format!("Hi {},", n.trim()),
        _ => "Hi,".to_string(),
    };

    let text = format!(
        "{greeting}\n\n\
         Your blend is ready to collect: {what}\n\n\
         Every bottle is mixed by hand at the bar, which is why this one took a \
         little while. Come by whenever suits — no need to book.\n\n\
         See your blends: {portal_url}\n\n\
         The Blend Bar, Oklahoma City"
    );

    let html = wrap(
        "Your blend is ready",
        &format!(
            r#"<p style="margin:0 0 12px;font-size:15px;line-height:1.6;">{}</p>
    <p style="margin:0 0 8px;font-size:15px;line-height:1.6;">Your blend is ready to collect:</p>
    <p style="margin:0 0 16px;padding:12px 16px;background:#f1e9da;border-radius:3px;font-size:15px;color:#211c15;">{}</p>
    <p style="margin:0;font-size:14px;line-height:1.6;color:#897a63;">Every bottle is mixed by hand at the bar, which is why this one took a little while. Come by whenever suits &mdash; no need to book.</p>
    {}"#,
            esc(&greeting),
            esc(what),
            button(portal_url, "See your blends")
        ),
    );

    Rendered {
        subject: "Your blend is ready to collect".to_string(),
        text,
        html,
    }
}

/// Sent by the admin "send test" button.
pub fn test_message(site: &str) -> Rendered {
    let text = format!(
        "This is a test from The Blend Bar app.\n\n\
         If you're reading this, outbound email is working: the app can reach the \
         Google Workspace relay and your domain accepted the message.\n\n{site}"
    );
    let html = wrap(
        "Email is working",
        &format!(
            r#"<p style="margin:0 0 12px;font-size:15px;line-height:1.6;">This is a test from The Blend Bar app.</p>
    <p style="margin:0;font-size:14px;line-height:1.6;color:#897a63;">If you&rsquo;re reading this, outbound email is working: the app reached the Google Workspace relay and your domain accepted the message. <a href="{}" style="color:#8c6a36;">{}</a></p>"#,
            esc(site),
            esc(site)
        ),
    );
    Rendered {
        subject: "The Blend Bar — email test".to_string(),
        text,
        html,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_link_carries_the_link_in_both_parts() {
        // A client that strips HTML must still be able to sign in.
        let link = "https://sandbox.theblendbarokc.com/portal/verify?token=abc123";
        let r = magic_link(link, 15);
        assert!(r.text.contains(link), "plain text lost the link");
        assert!(r.html.contains(link), "html lost the link");
        assert!(r.text.contains("15 minutes"));
    }

    #[test]
    fn magic_link_says_what_to_do_if_unexpected() {
        // Anyone can trigger a sign-in mail for any address, so an unsolicited
        // one must not read as a break-in.
        let r = magic_link("https://x.test/t", 15);
        assert!(r.text.to_lowercase().contains("ignore"));
        assert!(r.html.to_lowercase().contains("ignore"));
    }

    #[test]
    fn html_escapes_user_supplied_text() {
        // Blend and customer names are typed by staff and reach the markup.
        let r = order_ready(
            Some("<script>alert(1)</script>"),
            "Amber & Oud <3.4 oz>",
            "https://x.test/portal",
        );
        assert!(!r.html.contains("<script>"), "name was not escaped");
        assert!(r.html.contains("&lt;script&gt;"));
        assert!(r.html.contains("Amber &amp; Oud"));
    }

    #[test]
    fn order_ready_greets_by_name_when_there_is_one() {
        assert!(order_ready(Some("Alex"), "x", "u").text.starts_with("Hi Alex,"));
        assert!(order_ready(None, "x", "u").text.starts_with("Hi,"));
        // A blank name must not produce "Hi ,".
        assert!(order_ready(Some("   "), "x", "u").text.starts_with("Hi,"));
    }

    #[test]
    fn every_message_has_a_subject_and_both_bodies() {
        for r in [
            magic_link("https://x.test/t", 15),
            order_ready(Some("A"), "Blend", "https://x.test/p"),
            test_message("https://x.test"),
        ] {
            assert!(!r.subject.is_empty());
            assert!(!r.text.trim().is_empty());
            assert!(r.html.contains("</div>"));
        }
    }
}
