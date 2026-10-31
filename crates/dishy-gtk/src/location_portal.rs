//! "Use this device location" through the XDG Location portal (spec 0180).
//!
//! The browser's geolocation has no Linux desktop counterpart in GTK, so this
//! asks the portal, which shows the desktop's own permission prompt and works
//! inside the Flatpak sandbox without a finish-arg. D-Bus only; no HTTP.

use dishy_sky::ObserverLocation;
use gtk::gio;
use gtk::glib;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const LOCATION: &str = "org.freedesktop.portal.Location";
/// Electron's geolocation timeout.
const TIMEOUT: Duration = Duration::from_secs(15);
/// The portal's `GEOCLUE_ACCURACY_LEVEL_CITY`: look angles need ~km.
const ACCURACY_CITY: u32 = 4;

/// A LocationUpdated payload as an observer location. The portal reports an
/// unknown altitude as `-DBL_MAX`, which reads as sea level.
pub fn location_from_vardict(location: &glib::Variant) -> Option<ObserverLocation> {
    let dict = glib::VariantDict::new(Some(location));
    let number = |key: &str| {
        dict.lookup::<f64>(key)
            .ok()
            .flatten()
            .filter(|value| value.is_finite())
    };
    let altitude = number("Altitude")
        .filter(|metres| metres.abs() < 100_000.0)
        .unwrap_or(0.0);
    let found = ObserverLocation {
        latitude_deg: number("Latitude")?,
        longitude_deg: number("Longitude")?,
        altitude_m: altitude,
    };
    found.is_valid().then_some(found)
}

/// The handle token and the object paths the portal will use for it.
#[derive(Debug)]
struct PortalPaths {
    token: String,
    session: String,
    request: String,
}

impl PortalPaths {
    fn new(unique_name: &str, serial: u32) -> Self {
        let sender = unique_name.trim_start_matches(':').replace('.', "_");
        let token = format!("dishylink{serial}");
        Self {
            session: format!("{PORTAL_PATH}/session/{sender}/{token}"),
            request: format!("{PORTAL_PATH}/request/{sender}/{token}"),
            token,
        }
    }
}

/// A Request.Response signal's `(u a{sv})`: 0 is success.
fn response_succeeded(parameters: &glib::Variant) -> bool {
    parameters.n_children() > 0 && parameters.child_value(0).get::<u32>() == Some(0)
}

type Done = Rc<RefCell<Option<Box<dyn FnOnce(Result<ObserverLocation, ()>)>>>>;

/// Everything to undo when the request ends, however it ends.
#[derive(Default)]
struct Cleanup {
    connection: Option<gio::DBusConnection>,
    subscriptions: Vec<gio::SignalSubscriptionId>,
    session: Option<String>,
}

fn finish(done: &Done, cleanup: &Rc<RefCell<Cleanup>>, result: Result<ObserverLocation, ()>) {
    let Some(on_done) = done.borrow_mut().take() else {
        return;
    };
    let mut cleanup = cleanup.borrow_mut();
    if let Some(connection) = cleanup.connection.take() {
        #[allow(deprecated)]
        for id in cleanup.subscriptions.drain(..) {
            connection.signal_unsubscribe(id);
        }
        if let Some(session) = cleanup.session.take() {
            connection.call(
                Some(PORTAL_BUS),
                &session,
                "org.freedesktop.portal.Session",
                "Close",
                None,
                None,
                gio::DBusCallFlags::NONE,
                -1,
                None::<&gio::Cancellable>,
                |_| {},
            );
        }
    }
    on_done(result);
}

/// Ask the portal for one fix and report it once. Failure, refusal, timeout,
/// or a missing portal all report `Err(())`.
pub fn request_location(on_done: impl FnOnce(Result<ObserverLocation, ()>) + 'static) {
    let done: Done = Rc::new(RefCell::new(Some(Box::new(on_done))));
    let cleanup = Rc::new(RefCell::new(Cleanup::default()));
    {
        let (done, cleanup) = (done.clone(), cleanup.clone());
        glib::timeout_add_local_once(TIMEOUT, move || finish(&done, &cleanup, Err(())));
    }
    glib::MainContext::default().spawn_local(async move {
        if start(done.clone(), cleanup.clone()).await.is_none() {
            finish(&done, &cleanup, Err(()));
        }
    });
}

/// The portal's session handshake. `None` on any failure before a fix.
async fn start(done: Done, cleanup: Rc<RefCell<Cleanup>>) -> Option<()> {
    let connection = gio::bus_get_future(gio::BusType::Session).await.ok()?;
    cleanup.borrow_mut().connection = Some(connection.clone());
    // A fresh token per request, so a second request never reuses a session
    // path the portal may still be closing.
    static SERIAL: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let PortalPaths {
        token,
        session: session_path,
        request: request_path,
    } = PortalPaths::new(connection.unique_name()?.as_str(), serial);

    // Subscribe before asking, so the first fix cannot slip past.
    let (fix_done, fix_cleanup, wanted_session) =
        (done.clone(), cleanup.clone(), session_path.clone());
    #[allow(deprecated)]
    let updates = connection.signal_subscribe(
        Some(PORTAL_BUS),
        Some(LOCATION),
        Some("LocationUpdated"),
        Some(PORTAL_PATH),
        None,
        gio::DBusSignalFlags::NONE,
        move |_, _, _, _, _, parameters| {
            let session = parameters.child_value(0).str().map(str::to_owned);
            if session.as_deref() != Some(wanted_session.as_str()) {
                return;
            }
            let result = location_from_vardict(&parameters.child_value(1)).ok_or(());
            finish(&fix_done, &fix_cleanup, result);
        },
    );
    let (refuse_done, refuse_cleanup) = (done.clone(), cleanup.clone());
    #[allow(deprecated)]
    let responses = connection.signal_subscribe(
        Some(PORTAL_BUS),
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(&request_path),
        None,
        gio::DBusSignalFlags::NONE,
        move |_, _, _, _, _, parameters| {
            // On success the fix then arrives as LocationUpdated.
            if !response_succeeded(parameters) {
                finish(&refuse_done, &refuse_cleanup, Err(()));
            }
        },
    );
    cleanup
        .borrow_mut()
        .subscriptions
        .extend([updates, responses]);

    let options = glib::VariantDict::new(None);
    options.insert("session_handle_token", token.as_str());
    options.insert("accuracy", ACCURACY_CITY);
    connection
        .call_future(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            LOCATION,
            "CreateSession",
            Some(&glib::Variant::tuple_from_iter([options.end()])),
            None,
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await
        .ok()?;
    cleanup.borrow_mut().session = Some(session_path.clone());

    let start_options = glib::VariantDict::new(None);
    start_options.insert("handle_token", token.as_str());
    let session = glib::variant::ObjectPath::try_from(session_path).ok()?;
    connection
        .call_future(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            LOCATION,
            "Start",
            Some(&glib::Variant::tuple_from_iter([
                glib::variant::ToVariant::to_variant(&session),
                glib::variant::ToVariant::to_variant(""),
                start_options.end(),
            ])),
            None,
            gio::DBusCallFlags::NONE,
            -1,
        )
        .await
        .ok()?;
    Some(())
}

#[cfg(test)]
mod tests {
    use glib::variant::ToVariant;

    use super::*;

    fn vardict(entries: &[(&str, glib::Variant)]) -> glib::Variant {
        let dict = glib::VariantDict::new(None);
        for (key, value) in entries {
            dict.insert_value(key, value);
        }
        dict.end()
    }

    // SPEC-LOC-002: the handshake's object paths follow the portal's
    // convention, each request gets its own token, and only response 0 counts
    // as success.
    #[test]
    fn handshake_paths_and_responses() {
        let first = PortalPaths::new(":1.42", 7);
        assert_eq!(first.token, "dishylink7");
        assert_eq!(
            first.session,
            "/org/freedesktop/portal/desktop/session/1_42/dishylink7"
        );
        assert_eq!(
            first.request,
            "/org/freedesktop/portal/desktop/request/1_42/dishylink7"
        );
        assert_ne!(PortalPaths::new(":1.42", 8).session, first.session);
        assert!(response_succeeded(
            &(0_u32, glib::VariantDict::new(None).end()).to_variant()
        ));
        assert!(!response_succeeded(
            &(1_u32, glib::VariantDict::new(None).end()).to_variant()
        ));
        assert!(!response_succeeded(
            &(2_u32, glib::VariantDict::new(None).end()).to_variant()
        ));
    }

    // SPEC-LOC-003: coordinates and a finite altitude become the observer
    // location; a payload without valid coordinates is refused.
    #[test]
    fn portal_payloads_become_observer_locations() {
        let fix = vardict(&[
            ("Latitude", 6.5244_f64.to_variant()),
            ("Longitude", 3.3792_f64.to_variant()),
            ("Altitude", 41.0_f64.to_variant()),
            ("Accuracy", 30.0_f64.to_variant()),
        ]);
        let location = location_from_vardict(&fix).expect("a fix");
        assert_eq!(
            (
                location.latitude_deg,
                location.longitude_deg,
                location.altitude_m
            ),
            (6.5244, 3.3792, 41.0)
        );

        let unknown_altitude = vardict(&[
            ("Latitude", 6.5_f64.to_variant()),
            ("Longitude", 3.3_f64.to_variant()),
            ("Altitude", (-f64::MAX).to_variant()),
        ]);
        assert_eq!(
            location_from_vardict(&unknown_altitude).map(|location| location.altitude_m),
            Some(0.0)
        );
        assert!(
            location_from_vardict(&vardict(&[
                ("Latitude", 95.0_f64.to_variant()),
                ("Longitude", 0.0_f64.to_variant())
            ]))
            .is_none()
        );
        assert!(location_from_vardict(&vardict(&[("Accuracy", 5.0_f64.to_variant())])).is_none());
    }
}
