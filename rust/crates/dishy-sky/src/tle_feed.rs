//! The live constellation feed's state: when to fetch, how to back off, and
//! what the page should say meanwhile.
//!
//! This is a pure state machine. The host performs the HTTP request (through
//! `dishy-core`) on a worker thread and reports the outcome here, so the
//! retry and freshness rules can be tested with a fake clock and this crate
//! keeps no network dependency (SPEC-CONST-004).

use crate::constellation::{ObserverLocation, TleCatalog};

/// Delays between attempts after a failure, as in `hooks/useSatellites.ts`.
pub const RETRY_DELAYS_MS: [i64; 4] = [5_000, 15_000, 60_000, 300_000];

/// A catalog older than this is refetched. A stale catalog stays in use
/// while the refetch is pending or failing.
pub const TLE_MAX_AGE_MS: i64 = 6 * 3_600_000;

/// Which side of the fetch failed, because each gets different words.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TleFeedError {
    /// The request could not be made: offline, DNS, timeout, connection dropped.
    Offline,
    /// The source answered, but not with a usable catalog.
    Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TleFeedState {
    /// No observer location, so there is nothing to compute.
    LocationNeeded,
    /// The first catalog has not arrived yet.
    Loading,
    /// A catalog is available.
    Active,
    /// The last attempt failed and no catalog is available. A retry is scheduled.
    Error(TleFeedError),
}

#[derive(Default)]
pub struct TleFeed {
    observer: Option<ObserverLocation>,
    catalog: Option<TleCatalog>,
    fetched_at_ms: Option<i64>,
    failures: usize,
    retry_at_ms: Option<i64>,
    in_flight: bool,
    last_error: Option<TleFeedError>,
}

impl TleFeed {
    pub fn new() -> Self {
        Self::default()
    }

    /// A cached catalog from a previous run, used if it is still fresh.
    pub fn with_cached(text: &str, fetched_at_ms: i64) -> Self {
        let mut feed = Self::new();
        if let Ok(catalog) = TleCatalog::parse(text) {
            feed.catalog = Some(catalog);
            feed.fetched_at_ms = Some(fetched_at_ms);
        }
        feed
    }

    pub fn set_observer(&mut self, observer: Option<ObserverLocation>) {
        self.observer = observer.filter(|observer| observer.is_valid());
    }

    pub fn observer(&self) -> Option<ObserverLocation> {
        self.observer
    }

    pub fn catalog(&self) -> Option<&TleCatalog> {
        self.catalog.as_ref()
    }

    pub fn state(&self) -> TleFeedState {
        if self.observer.is_none() {
            TleFeedState::LocationNeeded
        } else if self.catalog.is_some() {
            TleFeedState::Active
        } else if let Some(error) = self.last_error
            && !self.in_flight
        {
            TleFeedState::Error(error)
        } else {
            TleFeedState::Loading
        }
    }

    /// Whether the host should start a fetch now. It is never true without a
    /// location, while a fetch is in flight, or before the backoff expires.
    pub fn wants_fetch(&self, now_ms: i64) -> bool {
        if self.observer.is_none() || self.in_flight {
            return false;
        }
        if self.retry_at_ms.is_some_and(|retry_at| now_ms < retry_at) {
            return false;
        }
        match self.fetched_at_ms {
            Some(fetched_at) if self.catalog.is_some() => now_ms - fetched_at >= TLE_MAX_AGE_MS,
            _ => true,
        }
    }

    pub fn begin_fetch(&mut self) {
        self.in_flight = true;
    }

    /// Report the fetch outcome. Returns whether the text was accepted, so the
    /// host caches only a catalog that parsed.
    pub fn finish_fetch(&mut self, result: Result<String, TleFeedError>, now_ms: i64) -> bool {
        self.in_flight = false;
        let parsed = result.and_then(|text| {
            if !text.contains("\n1 ") {
                return Err(TleFeedError::Source);
            }
            TleCatalog::parse(&text).map_err(|_| TleFeedError::Source)
        });
        match parsed {
            Ok(catalog) => {
                self.catalog = Some(catalog);
                self.fetched_at_ms = Some(now_ms);
                self.failures = 0;
                self.retry_at_ms = None;
                self.last_error = None;
                true
            }
            Err(error) => {
                let delay = RETRY_DELAYS_MS[self.failures.min(RETRY_DELAYS_MS.len() - 1)];
                self.failures += 1;
                self.retry_at_ms = Some(now_ms + delay);
                self.last_error = Some(error);
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ISS_TLE: &str = "ISS (ZARYA)\n\
1 25544U 98067A   20194.88612269 -.00002218  00000-0 -31515-4 0  9992\n\
2 25544  51.6461 221.2784 0001413  89.1723 280.4612 15.49507896236008\n";

    const SITE: ObserverLocation = ObserverLocation {
        latitude_deg: 4.8172,
        longitude_deg: 6.9679,
        altitude_m: 0.0,
    };

    // SPEC-CONST-005: no location, no fetch. A location starts the first fetch.
    // Failures back off on the Electron schedule and name the failing side.
    #[test]
    fn the_feed_waits_for_a_location_and_backs_off_on_failure() {
        let mut feed = TleFeed::new();
        assert_eq!(feed.state(), TleFeedState::LocationNeeded);
        assert!(!feed.wants_fetch(0));

        feed.set_observer(Some(SITE));
        assert_eq!(feed.state(), TleFeedState::Loading);
        assert!(feed.wants_fetch(0));
        feed.begin_fetch();
        assert!(!feed.wants_fetch(0), "one request at a time");

        assert!(!feed.finish_fetch(Err(TleFeedError::Offline), 1_000));
        assert_eq!(feed.state(), TleFeedState::Error(TleFeedError::Offline));
        assert!(!feed.wants_fetch(5_999));
        assert!(feed.wants_fetch(6_000));

        feed.begin_fetch();
        assert!(!feed.finish_fetch(Ok("<html>busy</html>".into()), 6_000));
        assert_eq!(feed.state(), TleFeedState::Error(TleFeedError::Source));
        assert!(!feed.wants_fetch(20_999));
        assert!(feed.wants_fetch(21_000));

        for _ in 0..5 {
            feed.begin_fetch();
            feed.finish_fetch(Err(TleFeedError::Offline), 0);
        }
        assert!(
            !feed.wants_fetch(299_999),
            "the longest delay is the ceiling"
        );
        assert!(feed.wants_fetch(300_000));

        feed.begin_fetch();
        assert!(feed.finish_fetch(Ok(ISS_TLE.into()), 400_000));
        assert_eq!(feed.state(), TleFeedState::Active);
        assert_eq!(feed.catalog().map(TleCatalog::satellite_count), Some(1));
    }

    // SPEC-CONST-005: a fresh catalog is not refetched. A stale one is, and it
    // stays in use while the refetch fails. Clearing the location hides it.
    #[test]
    fn a_fresh_catalog_is_reused_and_a_stale_one_survives_a_failed_refetch() {
        let mut feed = TleFeed::with_cached(ISS_TLE, 0);
        feed.set_observer(Some(SITE));
        assert_eq!(feed.state(), TleFeedState::Active);
        assert!(!feed.wants_fetch(TLE_MAX_AGE_MS - 1));
        assert!(feed.wants_fetch(TLE_MAX_AGE_MS));

        feed.begin_fetch();
        feed.finish_fetch(Err(TleFeedError::Offline), TLE_MAX_AGE_MS);
        assert_eq!(feed.state(), TleFeedState::Active);

        feed.set_observer(None);
        assert_eq!(feed.state(), TleFeedState::LocationNeeded);
        assert!(!feed.wants_fetch(i64::MAX));

        let invalid = ObserverLocation {
            latitude_deg: 95.0,
            ..SITE
        };
        feed.set_observer(Some(invalid));
        assert_eq!(feed.state(), TleFeedState::LocationNeeded);
    }
}
