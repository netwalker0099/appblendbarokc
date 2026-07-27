//! A small in-memory fixed-window rate limiter.
//!
//! Exists for one endpoint: the public share-page checkout. That endpoint takes
//! no authentication, writes rows (customer, order, cart), and makes an outbound
//! Square call — so without a cap, anyone with the URL could fill the database
//! and burn Square API quota from a laptop.
//!
//! Deliberately in-process and approximate. A single API process serves this app,
//! so a shared store would add a dependency for no benefit; and the goal is to
//! stop casual abuse, not to be exact at the boundary. If the app is ever run
//! multi-process, this becomes per-process and the limit effectively multiplies —
//! worth remembering before scaling out.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Stop the map growing without bound under a distributed flood. Once past this,
/// a sweep drops every key with no live hits; if that isn't enough, the oldest
/// entries go. Losing state only ever *forgives* a caller, never over-blocks.
const MAX_TRACKED_KEYS: usize = 10_000;

pub struct RateLimiter {
    window: Duration,
    max_in_window: usize,
    hits: Mutex<HashMap<String, Vec<Instant>>>,
}

impl RateLimiter {
    pub fn new(max_in_window: usize, window: Duration) -> Self {
        Self {
            window,
            max_in_window,
            hits: Mutex::new(HashMap::new()),
        }
    }

    /// Record an attempt for `key`. Returns true when it is allowed.
    pub fn check(&self, key: &str) -> bool {
        let now = Instant::now();
        let mut hits = self.hits.lock().expect("rate limiter poisoned");

        if hits.len() > MAX_TRACKED_KEYS {
            let window = self.window;
            hits.retain(|_, times| times.iter().any(|t| now.duration_since(*t) < window));
            if hits.len() > MAX_TRACKED_KEYS {
                hits.clear();
                tracing::warn!("rate limiter tracking table cleared under load");
            }
        }

        let entry = hits.entry(key.to_string()).or_default();
        entry.retain(|t| now.duration_since(*t) < self.window);

        if entry.len() >= self.max_in_window {
            return false;
        }
        entry.push(now);
        true
    }
}

/// The caller's IP as seen through Caddy.
///
/// The API is never published directly (compose uses `expose`, not `ports`), so
/// the only thing that can reach it is Caddy, and Caddy *appends* the immediate
/// peer to any inbound `X-Forwarded-For`. The rightmost entry is therefore the
/// one Caddy wrote and the only one that isn't client-controlled — a client
/// sending `X-Forwarded-For: 1.2.3.4` just gets their real address appended after
/// it. Taking the leftmost, as is common, would let anyone forge a fresh identity
/// per request and walk straight through the limit.
pub fn client_key(headers: &axum::http::HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.rsplit(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        // No header means something other than Caddy reached us. Bucket those
        // together rather than treating them as unlimited.
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_up_to_the_limit_then_blocks() {
        let rl = RateLimiter::new(3, Duration::from_secs(60));
        assert!(rl.check("a"));
        assert!(rl.check("a"));
        assert!(rl.check("a"));
        assert!(!rl.check("a"), "fourth call in the window must be blocked");
    }

    #[test]
    fn keys_are_independent() {
        let rl = RateLimiter::new(1, Duration::from_secs(60));
        assert!(rl.check("a"));
        assert!(!rl.check("a"));
        assert!(rl.check("b"), "one caller must not exhaust another's budget");
    }

    #[test]
    fn the_window_expires() {
        let rl = RateLimiter::new(1, Duration::from_millis(50));
        assert!(rl.check("a"));
        assert!(!rl.check("a"));
        std::thread::sleep(Duration::from_millis(70));
        assert!(rl.check("a"), "budget must refill once the window passes");
    }

    #[test]
    fn takes_the_rightmost_forwarded_for() {
        // A client forging a header must not be able to mint a new identity:
        // Caddy appends the real peer on the right.
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-forwarded-for", "1.2.3.4, 203.0.113.9".parse().unwrap());
        assert_eq!(client_key(&h), "203.0.113.9");
    }

    #[test]
    fn handles_a_single_forwarded_for_and_a_missing_one() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-forwarded-for", "203.0.113.9".parse().unwrap());
        assert_eq!(client_key(&h), "203.0.113.9");
        assert_eq!(client_key(&axum::http::HeaderMap::new()), "unknown");
    }
}
