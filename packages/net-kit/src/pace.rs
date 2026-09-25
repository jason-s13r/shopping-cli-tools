//! Spacing requests out, and backing off when a site says to slow down.
//!
//! The timing is `governor` (steady spacing) and `backon` (Fibonacci retry);
//! this module only decides what counts as a throttle and how long to wait.

use std::time::Duration;

use backon::{FibonacciBuilder, Retryable};
use governor::{DefaultDirectRateLimiter, Jitter, Quota, RateLimiter};

/// How fast a client may go, and how patiently it retries a throttle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pace {
    interval: Duration,
    retries: usize,
    max_backoff: Duration,
}

impl Pace {
    /// One request per `interval`, retrying a throttle four times.
    pub fn new(interval: Duration) -> Pace {
        Pace {
            interval,
            retries: 4,
            max_backoff: Duration::from_secs(30),
        }
    }

    /// No spacing and no retries, for a mock server.
    pub fn off() -> Pace {
        Pace {
            interval: Duration::ZERO,
            retries: 0,
            max_backoff: Duration::ZERO,
        }
    }

    pub fn with_retries(mut self, retries: usize) -> Pace {
        self.retries = retries;
        self
    }

    /// The longest single wait. A `Retry-After` asking for more is not waited
    /// out: the throttle is handed back so the caller can say so.
    pub fn with_max_backoff(mut self, max_backoff: Duration) -> Pace {
        self.max_backoff = max_backoff;
        self
    }

    pub fn interval(&self) -> Duration {
        self.interval
    }
}

/// A [`Pace`] in force. Share one per host: its spacing covers every request
/// sent through it, concurrent ones included.
pub struct Pacer {
    pace: Pace,
    /// `None` for a zero interval, which `governor` cannot express.
    limiter: Option<DefaultDirectRateLimiter>,
}

enum Attempt {
    Throttled(Box<wreq::Response>),
    Transport(wreq::Error),
}

impl Pacer {
    pub fn new(pace: Pace) -> Pacer {
        let limiter = Quota::with_period(pace.interval).map(RateLimiter::direct);
        Pacer { pace, limiter }
    }

    pub fn pace(&self) -> &Pace {
        &self.pace
    }

    /// Wait for this client's next slot. Jittered, so requests do not land on
    /// an exact beat.
    pub async fn wait(&self) {
        if let Some(limiter) = &self.limiter {
            limiter
                .until_ready_with_jitter(Jitter::up_to(self.pace.interval / 4))
                .await;
        }
    }

    /// Send a request in its slot, retrying a throttle on a Fibonacci backoff.
    ///
    /// `build` is called once per attempt, because sending spends the builder.
    /// Only a throttle is retried -- a transport error is returned at once.
    /// A throttle that outlasts the retries comes back as the response it was,
    /// for the caller's usual status handling to name. `on_retry` hears the
    /// status and the wait before each retry.
    pub async fn send(
        &self,
        build: impl Fn() -> wreq::RequestBuilder,
        on_retry: impl Fn(u16, Duration),
    ) -> Result<wreq::Response, wreq::Error> {
        let build = &build;
        let attempt = move || async move {
            self.wait().await;
            let response = build().send().await.map_err(Attempt::Transport)?;
            if is_throttle(response.status().as_u16()) {
                Err(Attempt::Throttled(Box::new(response)))
            } else {
                Ok(response)
            }
        };

        let sent = attempt
            .retry(self.backoff())
            .when(|e| matches!(e, Attempt::Throttled(_)))
            .adjust(|e, next| match (e, next) {
                // The site's own number beats the schedule, unless it is longer
                // than this client is willing to sit still for.
                (Attempt::Throttled(r), Some(next)) => match retry_after(r.headers()) {
                    Some(asked) if asked > self.pace.max_backoff => None,
                    Some(asked) => Some(asked),
                    None => Some(next),
                },
                _ => None,
            })
            .notify(|e, wait| {
                if let Attempt::Throttled(r) = e {
                    on_retry(r.status().as_u16(), wait);
                }
            })
            .await;

        match sent {
            Ok(response) => Ok(response),
            Err(Attempt::Throttled(response)) => Ok(*response),
            Err(Attempt::Transport(e)) => Err(e),
        }
    }

    fn backoff(&self) -> FibonacciBuilder {
        FibonacciBuilder::default()
            .with_min_delay(self.pace.interval)
            .with_max_delay(self.pace.max_backoff)
            .with_max_times(self.pace.retries)
            .with_jitter()
    }
}

/// A status that means "slow down" rather than "no".
pub fn is_throttle(status: u16) -> bool {
    matches!(status, 429 | 503)
}

/// `Retry-After` in its delta-seconds form.
///
/// The HTTP-date form is ignored: no storefront here has been seen sending it,
/// and a guessed number would be worse than none.
pub fn retry_after(headers: &wreq::header::HeaderMap) -> Option<Duration> {
    headers
        .get(wreq::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()
        .map(Duration::from_secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn headers(value: &str) -> wreq::header::HeaderMap {
        let mut h = wreq::header::HeaderMap::new();
        h.insert(wreq::header::RETRY_AFTER, value.parse().unwrap());
        h
    }

    #[tokio::test]
    async fn off_never_waits() {
        let pacer = Pacer::new(Pace::off());
        let start = Instant::now();
        for _ in 0..50 {
            pacer.wait().await;
        }
        assert!(start.elapsed() < Duration::from_millis(20));
    }

    #[tokio::test]
    async fn requests_are_spaced_by_the_interval() {
        // Real time: governor sleeps on its own timer, not tokio's.
        let pacer = Pacer::new(Pace::new(Duration::from_millis(40)));
        let start = Instant::now();
        for _ in 0..3 {
            pacer.wait().await;
        }
        assert!(
            start.elapsed() >= Duration::from_millis(80),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn retry_after_reads_seconds_only() {
        assert_eq!(retry_after(&headers("7")), Some(Duration::from_secs(7)));
        assert_eq!(retry_after(&headers(" 12 ")), Some(Duration::from_secs(12)));
        assert_eq!(retry_after(&headers("soon")), None);
        assert_eq!(retry_after(&headers("Wed, 21 Oct 2026 07:28:00 GMT")), None);
        assert_eq!(retry_after(&wreq::header::HeaderMap::new()), None);
    }

    #[test]
    fn only_429_and_503_are_throttles() {
        assert!(is_throttle(429));
        assert!(is_throttle(503));
        assert!(!is_throttle(403));
        assert!(!is_throttle(500));
        assert!(!is_throttle(200));
    }
}
