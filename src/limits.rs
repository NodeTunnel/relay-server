//! Traffic limiting primitives. `now` is passed in so the window math can be
//! tested without sleeping.

use std::time::{Duration, Instant};

use crate::config::loader::Config;

/// Reliable drops allowed before the session is dropped.
/// Violations decay by one per second, so this caps a sustained overrun.
pub const MAX_TRAFFIC_VIOLATIONS: u32 = 20;

/// Estimated framing overhead. The real size is only known after encoding.
pub const WIRE_OVERHEAD_ESTIMATE: u64 = 32;

/// Time-sliced byte budget: `max_per_window` bytes per `interval`.
#[derive(Debug)]
pub struct BandwidthLimiter {
    interval: Duration,
    max_per_window: u64,
    used: u64,
    window_start: Instant,
}

impl BandwidthLimiter {
    pub fn per_second(bytes_per_sec: u64, interval: Duration, now: Instant) -> Self {
        let max_per_window =
            u64::try_from(u128::from(bytes_per_sec) * interval.as_millis() / 1000).unwrap_or(u64::MAX);

        Self::per_window(max_per_window.max(1), interval, now)
    }

    /// A total per `window`, rather than a rate.
    pub fn per_window(max_bytes: u64, window: Duration, now: Instant) -> Self {
        Self {
            interval: window,
            max_per_window: max_bytes,
            used: 0,
            window_start: now,
        }
    }

    pub fn window_allowance(&self) -> u64 {
        self.max_per_window
    }

    /// A packet bigger than the whole allowance is accepted on an empty window;
    /// otherwise it could never be sent at all.
    pub fn would_fit(&self, bytes: u64, now: Instant) -> bool {
        if self.window_expired(now) {
            return true;
        }

        self.used == 0 || self.used.saturating_add(bytes) <= self.max_per_window
    }

    pub fn consume(&mut self, bytes: u64, now: Instant) {
        if self.window_expired(now) {
            self.window_start = now;
            self.used = bytes;
            return;
        }

        self.used = self.used.saturating_add(bytes);
    }

    fn window_expired(&self, now: Instant) -> bool {
        now.duration_since(self.window_start) > self.interval
    }
}

/// Every traffic limit, resolved from config once at startup.
#[derive(Clone, Copy, Debug)]
pub struct TrafficLimits {
    pub client_bytes_per_sec: u64,
    pub global_bytes_per_sec: u64,
    pub daily_bytes: u64,
    pub daily_control_bytes: u64,
    pub interval: Duration,
    pub max_session_bytes: u64,
    pub max_session_duration: Duration,
}

impl TrafficLimits {
    pub fn from_config(config: &Config) -> Self {
        Self {
            client_bytes_per_sec: config.max_client_bytes_per_sec,
            global_bytes_per_sec: config.max_global_bytes_per_sec,
            daily_bytes: config.max_daily_bytes,
            daily_control_bytes: config.max_daily_control_bytes,
            interval: Duration::from_millis(config.traffic_interval_ms),
            max_session_bytes: config.max_session_bytes,
            max_session_duration: Duration::from_secs(config.max_session_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn limits_at(now: Instant) -> BandwidthLimiter {
        BandwidthLimiter::per_second(1000, Duration::from_secs(1), now)
    }

    /// Check first, charge only if it passed — as `PaperInterface::send` does.
    trait TestConsume {
        fn try_consume(&mut self, bytes: u64, now: Instant) -> bool;
    }

    impl TestConsume for BandwidthLimiter {
        fn try_consume(&mut self, bytes: u64, now: Instant) -> bool {
            if !self.would_fit(bytes, now) {
                return false;
            }

            self.consume(bytes, now);
            true
        }
    }

    #[test]
    fn allows_traffic_under_the_limit() {
        let now = Instant::now();
        let mut limiter = limits_at(now);

        assert!(limiter.try_consume(400, now));
        assert!(limiter.try_consume(400, now));
    }

    #[test]
    fn rejects_traffic_over_the_limit() {
        let now = Instant::now();
        let mut limiter = limits_at(now);

        assert!(limiter.try_consume(900, now));
        assert!(!limiter.try_consume(200, now));
    }

    #[test]
    fn rejecting_does_not_charge_the_budget() {
        let now = Instant::now();
        let mut limiter = limits_at(now);

        assert!(limiter.try_consume(900, now));
        assert!(!limiter.try_consume(200, now));
        assert!(limiter.try_consume(100, now));
    }

    #[test]
    fn resets_after_the_window_expires() {
        let now = Instant::now();
        let mut limiter = limits_at(now);

        assert!(limiter.try_consume(1000, now));
        assert!(!limiter.try_consume(1, now));

        let later = now + Duration::from_millis(1500);
        assert!(limiter.try_consume(1000, later));
    }

    #[test]
    fn charges_the_rollover_packet_to_the_new_window() {
        let now = Instant::now();
        let mut limiter = limits_at(now);

        assert!(limiter.try_consume(1000, now));

        // The rollover packet is charged to the new window.
        let later = now + Duration::from_millis(1500);
        assert!(limiter.try_consume(1000, later));
        assert!(!limiter.try_consume(1, later));
    }

    #[test]
    fn oversized_packet_passes_on_an_empty_window() {
        let now = Instant::now();
        let mut limiter = limits_at(now);

        assert!(limiter.try_consume(5000, now));
        assert!(!limiter.try_consume(1, now));
    }

    #[test]
    fn per_window_is_a_total_not_a_rate() {
        let now = Instant::now();
        let mut daily = BandwidthLimiter::per_window(5000, Duration::from_secs(86400), now);

        assert!(daily.try_consume(4000, now));
        assert!(!daily.try_consume(2000, now + Duration::from_secs(3600)));
        assert!(daily.try_consume(2000, now + Duration::from_secs(90000)));
    }

    #[test]
    fn daily_budgets_do_not_share() {
        let now = Instant::now();
        let day = Duration::from_secs(86400);
        let mut gameplay = BandwidthLimiter::per_window(1000, day, now);
        let mut control = BandwidthLimiter::per_window(100, day, now);

        assert!(gameplay.try_consume(1000, now));
        assert!(!gameplay.try_consume(1, now));

        assert!(control.try_consume(100, now));

        assert!(!control.try_consume(1, now));
    }

    #[test]
    fn sub_second_intervals_slice_the_rate() {
        let now = Instant::now();
        let limiter =
            BandwidthLimiter::per_second(65536, Duration::from_millis(250), now);

        assert_eq!(limiter.window_allowance(), 16384);
    }

    #[test]
    fn allowance_never_rounds_down_to_zero() {
        let now = Instant::now();
        let limiter = BandwidthLimiter::per_second(1, Duration::from_millis(1), now);

        assert_eq!(limiter.window_allowance(), 1);
    }
}
