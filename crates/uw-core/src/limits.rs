//! Being told to go away, and for how long.
//!
//! Every provider here is an undocumented endpoint behind somebody's edge
//! network, and the interesting failure is not "the request failed" but "the
//! request failed *and the server said when to come back*". A poller that
//! throws that away keeps knocking through the whole penalty — which is rude,
//! and on a limiter that counts blocked requests it is also self-defeating:
//! the wait never elapses because we keep resetting it.
//!
//! Anthropic's usage endpoint is the case that prompted this. It is rate
//! limited per IP rather than per account — an unauthenticated request gets the
//! same 429 — and it answers with `Retry-After: 3600`, an hour. Our own
//! exponential backoff caps out at fifteen minutes, so without reading that
//! header the daemon could never wait long enough to be let back in.

use std::fmt;
use std::time::Duration;

use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::StatusCode;

/// Longest wait we will honour.
///
/// A server asking for a day would otherwise park a provider until the app is
/// restarted, and a tile that says "rate limited" for a day is indistinguishable
/// from one that is simply broken. Beyond this we come back early and take
/// another 429 rather than disappearing.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(3 * 3600);

/// A refusal that carries a time to come back.
#[derive(Debug, Clone)]
pub struct RateLimited {
    pub status: StatusCode,
    /// From `Retry-After`, clamped to [`MAX_RETRY_AFTER`]. `None` when the
    /// server refused without saying for how long.
    pub retry_after: Option<Duration>,
    /// What the provider is called, for the message on the tile.
    pub what: &'static str,
}

impl fmt::Display for RateLimited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} is rate limiting us ({})", self.what, self.status)?;
        match self.retry_after {
            // Phrased as the wait rather than as the failure. This lands on a
            // tile, where "try again in 58 minutes" is a status and "429 Too
            // Many Requests" is a puzzle.
            Some(d) => write!(f, "; retrying in {}", human(d)),
            None => Ok(()),
        }
    }
}

impl std::error::Error for RateLimited {}

/// Recognise a refusal worth waiting out.
///
/// 429 always. 503 only when it names a time: without `Retry-After` it is an
/// ordinary outage, and ordinary outages are what the exponential backoff is
/// already for.
pub fn rate_limited(
    status: StatusCode,
    headers: &HeaderMap,
    what: &'static str,
) -> Option<RateLimited> {
    let retry_after = parse_retry_after(headers);
    if status != StatusCode::TOO_MANY_REQUESTS
        && !(status == StatusCode::SERVICE_UNAVAILABLE && retry_after.is_some())
    {
        return None;
    }
    Some(RateLimited {
        status,
        retry_after,
        what,
    })
}

/// `Retry-After: 3600`. The HTTP-date form is legal too and nobody sends it;
/// an unparseable value is treated as absent rather than as zero, because
/// "come back immediately" is the one reading that cannot be right.
fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    let secs: u64 = headers.get(RETRY_AFTER)?.to_str().ok()?.trim().parse().ok()?;
    Some(Duration::from_secs(secs).min(MAX_RETRY_AFTER))
}

fn human(d: Duration) -> String {
    let secs = d.as_secs();
    match secs {
        0..=90 => format!("{secs}s"),
        // Switches at 59 minutes rather than 60, so the hour that Anthropic
        // actually asks for reads "1h" instead of "60m".
        91..=3539 => format!("{}m", (secs + 30) / 60),
        _ => format!("{}h", (secs + 1800) / 3600),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                reqwest::header::HeaderName::from_bytes(k.as_bytes()).unwrap(),
                v.parse().unwrap(),
            );
        }
        h
    }

    #[test]
    fn a_429_with_a_retry_after_carries_the_wait() {
        let limited = rate_limited(
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("retry-after", "3600")]),
            "Anthropic",
        )
        .expect("a 429 is a rate limit");
        assert_eq!(limited.retry_after, Some(Duration::from_secs(3600)));
        assert!(limited.to_string().contains("1h"), "{limited}");
    }

    #[test]
    fn a_429_without_one_is_still_a_rate_limit() {
        // Worth recognising even so: the tile should say "rate limited" rather
        // than print a status code, and the caller falls back to its own
        // backoff for the timing.
        let limited = rate_limited(StatusCode::TOO_MANY_REQUESTS, &HeaderMap::new(), "Anthropic")
            .expect("a 429 is a rate limit with or without the header");
        assert_eq!(limited.retry_after, None);
    }

    #[test]
    fn an_ordinary_outage_is_left_to_the_backoff() {
        assert!(rate_limited(StatusCode::SERVICE_UNAVAILABLE, &HeaderMap::new(), "x").is_none());
        assert!(rate_limited(StatusCode::INTERNAL_SERVER_ERROR, &HeaderMap::new(), "x").is_none());
        assert!(rate_limited(StatusCode::UNAUTHORIZED, &HeaderMap::new(), "x").is_none());
        // ...unless it names a time, which is a server asking to be left alone
        // rather than one that has fallen over.
        assert!(rate_limited(
            StatusCode::SERVICE_UNAVAILABLE,
            &headers(&[("retry-after", "30")]),
            "x"
        )
        .is_some());
    }

    #[test]
    fn an_absurd_wait_is_clamped_rather_than_obeyed() {
        let limited = rate_limited(
            StatusCode::TOO_MANY_REQUESTS,
            &headers(&[("retry-after", "86400")]),
            "x",
        )
        .unwrap();
        assert_eq!(limited.retry_after, Some(MAX_RETRY_AFTER));
    }

    #[test]
    fn a_header_we_cannot_read_is_absent_not_zero() {
        // The HTTP-date form, and junk. Reading either as 0 would turn a
        // rate limit into a hot loop — the exact failure this module exists to
        // prevent.
        for value in ["Wed, 21 Oct 2026 07:28:00 GMT", "soon", "-5", ""] {
            let limited = rate_limited(
                StatusCode::TOO_MANY_REQUESTS,
                &headers(&[("retry-after", value)]),
                "x",
            )
            .unwrap();
            assert_eq!(limited.retry_after, None, "{value:?} was read as a duration");
        }
    }
}
