//! Desktop notification policy and rate limiting.
//!
//! The wording and the repeat rule live beside the TS implementation; this is
//! the same one-minute window from `core/alertNotification.ts`. Alert definitions
//! arrive in M10, but the throttle is part of the host shell and is enforced here
//! before any transport is added.

use std::collections::HashMap;

use dishy_core::alert::AlertTransition;

/// A minute, so a flapping link cannot become a storm.
pub const NOTIFICATION_THROTTLE_MS: i64 = 60_000;

/// Only an explicit yes announces anything. `None` means the user has never
/// chosen, which is not consent.
pub fn notifications_requested(wanted: Option<bool>) -> bool {
    wanted == Some(true)
}

pub fn should_notify_alert(wanted: Option<bool>, transition: &AlertTransition) -> bool {
    notifications_requested(wanted) && transition.is_notifiable()
}

#[derive(Debug, Clone)]
pub struct NotificationThrottle {
    window_ms: i64,
    last_sent_at: HashMap<String, i64>,
}

impl Default for NotificationThrottle {
    fn default() -> Self {
        Self::new()
    }
}

impl NotificationThrottle {
    pub fn new() -> Self {
        Self::with_window(NOTIFICATION_THROTTLE_MS)
    }

    pub fn with_window(window_ms: i64) -> Self {
        Self {
            window_ms,
            last_sent_at: HashMap::new(),
        }
    }

    /// Whether this key may be sent now. Records the send on a yes, so a caller
    /// cannot forget to reserve the slot.
    pub fn allow(&mut self, key: &str, now_ms: i64) -> bool {
        if let Some(last_sent_at) = self.last_sent_at.get(key)
            && now_ms - last_sent_at < self.window_ms
        {
            return false;
        }
        self.last_sent_at.insert(key.to_string(), now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SHELL-008: a condition crossing its threshold repeatedly reaches the
    // user once per minute, and an onset never swallows its own clear.
    #[test]
    fn flapping_link_notifies_once_per_window() {
        let mut throttle = NotificationThrottle::new();
        assert!(throttle.allow("alert-dish:obstruction", 1_000));
        assert!(!throttle.allow("alert-dish:obstruction", 30_000));
        assert!(!throttle.allow("alert-dish:obstruction", 60_999));
        assert!(throttle.allow("alert-dish:obstruction", 61_000));
    }

    #[test]
    fn keys_are_independent_and_clear_has_its_own_slot() {
        let mut throttle = NotificationThrottle::new();
        assert!(throttle.allow("alert-dish:offline", 1_000));
        assert!(throttle.allow("alert-router:offline", 1_000));
        assert!(throttle.allow("alert-dish:offline-cleared", 1_000));
        assert!(!notifications_requested(None));
        assert!(!notifications_requested(Some(false)));
        assert!(notifications_requested(Some(true)));
    }

    // SPEC-ALERT-006: only explicit opt-in and notifiable definitions may
    // reach the existing notification transport; a clear has an independent key.
    #[test]
    fn alert_notifications_require_explicit_opt_in_and_notifiable_definitions() {
        use dishy_core::alert::{AlertSource, AlertTransitionKind, Severity};

        let firing = AlertTransition {
            kind: AlertTransitionKind::Fired,
            source: AlertSource::Dish,
            key: "thermalShutdown".into(),
            severity: Severity::Critical,
            at_ms: 1,
            label: None,
        };
        let advisory = AlertTransition {
            key: "roaming".into(),
            severity: Severity::Advisory,
            ..firing.clone()
        };
        let cleared = AlertTransition {
            kind: AlertTransitionKind::Cleared,
            at_ms: 2,
            ..firing.clone()
        };
        assert!(!should_notify_alert(None, &firing));
        assert!(!should_notify_alert(Some(false), &firing));
        assert!(should_notify_alert(Some(true), &firing));
        assert!(!should_notify_alert(Some(true), &advisory));
        assert!(should_notify_alert(Some(true), &cleared));
        let mut throttle = NotificationThrottle::new();
        let firing_key = firing.notification_key();
        let cleared_key = cleared.notification_key();
        assert!(throttle.allow(&firing_key, 1_000));
        assert!(throttle.allow(&cleared_key, 1_000));
        assert!(!throttle.allow(&firing_key, 30_000));

        let monthly = AlertTransition {
            kind: AlertTransitionKind::Fired,
            source: AlertSource::System,
            key: "monthlyDataLimit:42".into(),
            severity: Severity::Warning,
            at_ms: 3,
            label: Some("Phone".into()),
        };
        let monthly_cleared = AlertTransition {
            kind: AlertTransitionKind::Cleared,
            at_ms: 4,
            ..monthly.clone()
        };
        // SPEC-METER-ALERT-004: explicit opt-in and distinct throttled keys
        // govern per-device firing and clearing announcements.
        assert!(!should_notify_alert(None, &monthly));
        assert!(should_notify_alert(Some(true), &monthly));
        assert_ne!(
            monthly.notification_key(),
            monthly_cleared.notification_key()
        );
        let mut monthly_throttle = NotificationThrottle::new();
        assert!(monthly_throttle.allow(&monthly.notification_key(), 1_000));
        assert!(monthly_throttle.allow(&monthly_cleared.notification_key(), 1_000));
        assert!(!monthly_throttle.allow(&monthly.notification_key(), 30_000));
    }
}
