//! The app's live status on D-Bus (spec 0189), for the Dishylink GNOME Shell
//! extension's Quick Settings pill. Read-only, and fed only by replies the app
//! already holds.

use gtk::gio;
use gtk::glib;
use gtk::glib::variant::ToVariant;

use dishy_core::telemetry::DishStatus;

use crate::i18n::Translator;
use crate::throughput::{THROUGHPUT_STALE_MS, ThroughputSample, format_rate};

pub const OBJECT_PATH: &str = "/com/dishylink/app/Status";
pub const INTERFACE: &str = "com.dishylink.Status1";

pub const INTROSPECTION: &str = r#"<node>
  <interface name="com.dishylink.Status1">
    <property name="Online" type="b" access="read"/>
    <property name="Connection" type="s" access="read"/>
    <property name="Country" type="s" access="read"/>
    <property name="UptimeS" type="t" access="read"/>
    <property name="DownBps" type="d" access="read"/>
    <property name="UpBps" type="d" access="read"/>
    <property name="LatencyMs" type="d" access="read"/>
    <property name="ActiveAlerts" type="u" access="read"/>
    <property name="Summary" type="s" access="read"/>
  </interface>
</node>"#;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct StatusProperties {
    pub online: bool,
    pub connection: String,
    pub country: String,
    pub uptime_s: u64,
    pub down_bps: f64,
    pub up_bps: f64,
    /// Negative when unknown.
    pub latency_ms: f64,
    pub active_alerts: u32,
    pub summary: String,
}

/// The inputs the pump already has, turned into the published properties.
#[allow(clippy::too_many_arguments)]
pub fn status_properties(
    reachable: Option<bool>,
    status: Option<&DishStatus>,
    sample: Option<&ThroughputSample>,
    latency_ms: Option<f64>,
    active_alerts: usize,
    now_ms: i64,
    translator: &Translator,
) -> StatusProperties {
    let (connection, word) = match reachable {
        Some(true) => ("online", "status.online"),
        Some(false) => ("unreachable", "status.dishUnreachable"),
        None => ("connecting", "status.connecting"),
    };
    let online = reachable == Some(true);
    // A stale feed reads as zero, as the tray's readout does.
    let (down_bps, up_bps) = sample
        .filter(|sample| now_ms.saturating_sub(sample.at_ms) < THROUGHPUT_STALE_MS)
        .map_or((0.0, 0.0), |sample| (sample.down_bps, sample.up_bps));
    let word = translator.text(word);
    let summary = if online {
        format!("{word} · ↓{}", format_rate(down_bps))
    } else {
        word
    };
    StatusProperties {
        online,
        connection: connection.into(),
        country: status
            .and_then(|status| status.device_info.as_ref())
            .and_then(|info| info.country_code.clone())
            .unwrap_or_default(),
        uptime_s: status
            .and_then(|status| status.device_state.as_ref())
            .and_then(|state| state.uptime_s)
            .map_or(0, |seconds| seconds.0),
        down_bps,
        up_bps,
        latency_ms: latency_ms.filter(|value| value.is_finite()).unwrap_or(-1.0),
        active_alerts: u32::try_from(active_alerts).unwrap_or(u32::MAX),
        summary,
    }
}

impl StatusProperties {
    /// Every property as `(name, value)`, in introspection order.
    pub fn entries(&self) -> Vec<(&'static str, glib::Variant)> {
        vec![
            ("Online", self.online.to_variant()),
            ("Connection", self.connection.to_variant()),
            ("Country", self.country.to_variant()),
            ("UptimeS", self.uptime_s.to_variant()),
            ("DownBps", self.down_bps.to_variant()),
            ("UpBps", self.up_bps.to_variant()),
            ("LatencyMs", self.latency_ms.to_variant()),
            ("ActiveAlerts", self.active_alerts.to_variant()),
            ("Summary", self.summary.to_variant()),
        ]
    }

    /// The properties whose values differ from `before`.
    pub fn changed_since(&self, before: &StatusProperties) -> Vec<(&'static str, glib::Variant)> {
        self.entries()
            .into_iter()
            .zip(before.entries())
            .filter(|((_, now), (_, then))| now != then)
            .map(|(entry, _)| entry)
            .collect()
    }
}

/// The exported object; `publish` updates it and signals what changed.
pub struct StatusBus {
    connection: gio::DBusConnection,
    current: std::rc::Rc<std::cell::RefCell<StatusProperties>>,
}

impl StatusBus {
    pub fn register(connection: &gio::DBusConnection) -> Result<Self, glib::Error> {
        let node = gio::DBusNodeInfo::for_xml(INTROSPECTION)?;
        let interface = node
            .lookup_interface(INTERFACE)
            .expect("the introspection declares the interface");
        let current = std::rc::Rc::new(std::cell::RefCell::new(StatusProperties::default()));
        let read = current.clone();
        connection
            .register_object(OBJECT_PATH, &interface)
            .property(move |_, _, _, _, name| {
                read.borrow()
                    .entries()
                    .into_iter()
                    .find(|(property, _)| *property == name)
                    .map(|(_, value)| value)
                    .unwrap_or_else(|| false.to_variant())
            })
            .build()?;
        Ok(Self {
            connection: connection.clone(),
            current,
        })
    }

    pub fn publish(&self, next: StatusProperties) {
        let changed = next.changed_since(&self.current.borrow());
        if changed.is_empty() {
            return;
        }
        *self.current.borrow_mut() = next;
        let dict = glib::VariantDict::new(None);
        for (name, value) in changed {
            dict.insert_value(name, &value);
        }
        let parameters = glib::Variant::tuple_from_iter([
            INTERFACE.to_variant(),
            dict.end(),
            Vec::<String>::new().to_variant(),
        ]);
        if let Err(error) = self.connection.emit_signal(
            None,
            OBJECT_PATH,
            "org.freedesktop.DBus.Properties",
            "PropertiesChanged",
            Some(&parameters),
        ) {
            tracing::debug!("status signal not sent: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Locale;

    fn status() -> DishStatus {
        DishStatus {
            device_info: Some(dishy_core::telemetry::DishDeviceInfo {
                country_code: Some("ZM".into()),
                ..Default::default()
            }),
            device_state: Some(dishy_core::telemetry::DeviceState {
                uptime_s: Some(dishy_core::telemetry::U64Value(59_220)),
            }),
            ..Default::default()
        }
    }

    // SPEC-PILL-001: the published properties come from the replies in hand.
    #[test]
    fn properties_map_from_the_replies_in_hand() {
        let english = Translator::new(Locale::En);
        let sample = ThroughputSample {
            at_ms: 10_000,
            down_bps: 2_900_000.0,
            up_bps: 162_000.0,
        };
        let props = status_properties(
            Some(true),
            Some(&status()),
            Some(&sample),
            Some(28.4),
            2,
            10_500,
            &english,
        );
        assert!(props.online);
        assert_eq!(props.connection, "online");
        assert_eq!(props.country, "ZM");
        assert_eq!(props.uptime_s, 59_220);
        assert_eq!((props.down_bps, props.up_bps), (2_900_000.0, 162_000.0));
        assert_eq!(props.latency_ms, 28.4);
        assert_eq!(props.active_alerts, 2);
        assert_eq!(
            props.summary,
            format!("online · ↓{}", format_rate(2_900_000.0))
        );

        let waiting = status_properties(None, None, None, None, 0, 0, &english);
        assert!(!waiting.online);
        assert_eq!(
            (waiting.connection.as_str(), waiting.summary.as_str()),
            ("connecting", "connecting")
        );
        assert_eq!(waiting.latency_ms, -1.0);
        let down = status_properties(Some(false), Some(&status()), None, None, 0, 0, &english);
        assert_eq!(
            (down.connection.as_str(), down.summary.as_str()),
            ("unreachable", "dish unreachable")
        );
    }

    // SPEC-PILL-002: only changed properties are signalled; a stale feed
    // reads as zero.
    #[test]
    fn only_changes_are_signalled() {
        let english = Translator::new(Locale::En);
        let sample = ThroughputSample {
            at_ms: 0,
            down_bps: 1_000_000.0,
            up_bps: 1_000.0,
        };
        let fresh = status_properties(Some(true), None, Some(&sample), None, 0, 1_000, &english);
        let stale = status_properties(
            Some(true),
            None,
            Some(&sample),
            None,
            0,
            THROUGHPUT_STALE_MS + 1,
            &english,
        );
        assert_eq!((stale.down_bps, stale.up_bps), (0.0, 0.0));
        let names: Vec<&str> = stale
            .changed_since(&fresh)
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["DownBps", "UpBps", "Summary"]);
        assert!(fresh.changed_since(&fresh.clone()).is_empty());
        // Online, Connection, DownBps, UpBps, an unknown (−1) LatencyMs, Summary.
        assert_eq!(fresh.changed_since(&StatusProperties::default()).len(), 6);
    }

    // SPEC-PILL-001: the introspection is a valid D-Bus interface with every
    // published property.
    #[test]
    fn the_interface_declares_every_property() {
        let node = gio::DBusNodeInfo::for_xml(INTROSPECTION).expect("valid XML");
        let interface = node.lookup_interface(INTERFACE).expect("interface");
        for (name, _) in StatusProperties::default().entries() {
            assert!(interface.lookup_property(name).is_some(), "{name}");
        }
    }
}
