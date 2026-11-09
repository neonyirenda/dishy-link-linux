//! Application wiring: one recorder, one window, one tray, one settings file.
//!
//! The `GtkApplication` provides the second-instance handshake. The recorder is
//! started on `startup` and owned by [`AppState`], not by a window, so closing the
//! window cannot stop collection. The tray is the only place quit is offered.

use std::cell::{Cell, RefCell};
use std::ffi::OsString;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use dishy_cloud::{
    CloudClient, ReqwestTransport, dish_config::DishConfigUpdate, router_config::RouterConfigUpdate,
};
use dishy_core::alert::{AlertSnapshot, AlertSource, AlertTransitionKind};
use dishy_core::calendar::now_ms;
use dishy_core::network::{
    ClientRateTracker, NetworkSnapshot, WifiClient, WifiConfig, build_nodes,
};
use dishy_core::telemetry::{DishObstructionMap, DishStatus, TelemetrySample};
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use libadwaita as adw;

use crate::activation::{Activation, ActivationGate, Launch};
use crate::alerts_view::AlertsPage;
use crate::alignment_view::AlignmentPage;
use crate::autostart::Autostart;
use crate::cloud_login::show_sign_in;
use crate::cloud_session::SecretToolSessionStore;
use crate::cloud_view::{AccountPage, CloudAction, CloudState};
use crate::dashboard_view::DashboardPage;
use crate::i18n::{Locale, Translator, detect_system_locale};
use crate::network_view::{NetworkCloudAction, NetworkPage};
use crate::notifications::{NotificationThrottle, notifications_requested, should_notify_alert};
use crate::observer_location;
use crate::paths::{APP_ID, HostPaths};
use crate::preferences::{PreferencesStore, Theme};
use crate::recorder::{RecorderCommand, RecorderThread, RecorderUpdate};
use crate::settings_view::{AppSettingsState, SettingsAction, SettingsPage};
use crate::sky_view::SkyPage;
use crate::speed_test_view::SpeedTestPage;
use crate::throughput::{ThroughputSample, live_readout};
use crate::tray::{DishyTray, TrayCommand, TrayState};
use crate::ui;
use crate::usage_view::DataUsagePage;
use crate::window_bounds::{Display, clamp_bounds};

const CLOUD_NETWORK_POLL_INTERVAL_MS: i64 = 15_000;

pub fn run() {
    init_tracing();

    let args: Vec<OsString> = std::env::args_os().collect();
    let launch = Launch::from_args(args.iter().filter_map(|arg| arg.to_str()));

    if let Err(error) = gtk::init() {
        eprintln!("dishy-gtk: could not initialize GTK: {error}");
        return;
    }
    if let Err(error) = adw::init() {
        eprintln!("dishy-gtk: could not initialize libadwaita: {error}");
        return;
    }

    let app = adw::Application::builder().application_id(APP_ID).build();
    let paths = HostPaths::discover();
    let preferences = PreferencesStore::load(paths.preferences_file());
    let locale = preferences
        .get()
        .locale
        .unwrap_or_else(detect_system_locale);
    let translator = Translator::new(locale);
    let state = Rc::new(AppState::new(
        app.clone(),
        paths,
        preferences,
        translator,
        launch.hidden,
    ));

    let startup_state = state.clone();
    app.connect_startup(move |_| startup_state.start());
    let activate_state = state.clone();
    app.connect_activate(move |_| activate_state.activate());
    let shutdown_state = state.clone();
    app.connect_shutdown(move |_| shutdown_state.shutdown());

    let quit_state = state.clone();
    let quit_action = gio::SimpleAction::new("quit", None);
    quit_action.connect_activate(move |_, _| quit_state.quit());
    app.add_action(&quit_action);

    let code = app.run_with_args(&Launch::gtk_args(args));
    if code.get() != 0 {
        std::process::exit(i32::from(code.get()));
    }
}

fn init_tracing() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| "dishy_core=info,dishy_gtk=info".into());
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

struct AppState {
    app: adw::Application,
    paths: HostPaths,
    preferences: RefCell<PreferencesStore>,
    translator: RefCell<Translator>,
    activation: RefCell<ActivationGate>,
    started: Cell<bool>,
    hold: RefCell<Option<gio::ApplicationHoldGuard>>,
    window: RefCell<Option<adw::ApplicationWindow>>,
    toolbar_layout: RefCell<Option<ui::ToolbarLayout>>,
    theme_button: RefCell<Option<gtk::Button>>,
    header_status: RefCell<Option<gtk::Label>>,
    connectivity_indicator: RefCell<Option<gtk::Label>>,
    glass: RefCell<Option<crate::theme::GlassBackdrop>>,
    status_bus: RefCell<Option<crate::status_bus::StatusBus>>,
    notification_badge: RefCell<Option<gtk::Label>>,
    dashboard: RefCell<Option<DashboardPage>>,
    dashboard_samples: RefCell<Arc<Vec<TelemetrySample>>>,
    sky: RefCell<Option<SkyPage>>,
    data_usage: RefCell<Option<DataUsagePage>>,
    alerts: RefCell<Option<AlertsPage>>,
    account: RefCell<Option<AccountPage>>,
    network: RefCell<Option<NetworkPage>>,
    speed_test: RefCell<Option<SpeedTestPage>>,
    alignment: RefCell<Option<AlignmentPage>>,
    network_cloud_actions: Sender<NetworkCloudAction>,
    network_cloud_action_receiver: Receiver<NetworkCloudAction>,
    settings: RefCell<Option<SettingsPage>>,
    latest_status: RefCell<Option<DishStatus>>,
    latest_alerts: RefCell<Option<AlertSnapshot>>,
    latest_network: RefCell<Option<Arc<NetworkSnapshot>>>,
    obstruction_map: RefCell<Option<DishObstructionMap>>,
    last_dashboard_refresh_ms: Cell<i64>,
    tray: RefCell<Option<ksni::blocking::Handle<DishyTray>>>,
    tray_commands: Sender<TrayCommand>,
    tray_receiver: Receiver<TrayCommand>,
    settings_actions: Sender<SettingsAction>,
    settings_receiver: Receiver<SettingsAction>,
    recorder: RefCell<Option<RecorderThread>>,
    recorder_updates: RefCell<Option<Receiver<RecorderUpdate>>>,
    latest_sample: Cell<Option<ThroughputSample>>,
    latest_router_reachable: Cell<Option<bool>>,
    tray_readout: RefCell<String>,
    notifier: RefCell<NotificationThrottle>,
    chimes: RefCell<crate::alert_sound::ChimeThrottle>,
    pump_started: Cell<bool>,
    cloud_actions: Sender<CloudAction>,
    cloud_action_receiver: Receiver<CloudAction>,
    cloud_requests: Sender<CloudRequest>,
    cloud_request_receiver: RefCell<Option<Receiver<CloudRequest>>>,
    cloud_updates: RefCell<Option<Receiver<CloudUpdate>>>,
    cloud_state: RefCell<CloudState>,
    login_window: RefCell<Option<adw::Window>>,
    network_cloud_demand: Cell<bool>,
    network_cloud_epoch: Cell<u64>,
    network_lan_reachable: Cell<Option<bool>>,
    network_roster_requested: Cell<bool>,
    network_cloud_roster_displayed: Cell<bool>,
    network_config_success_epoch: Cell<Option<u64>>,
    network_config_inflight: Cell<Option<u64>>,
    network_roster_inflight: Cell<Option<u64>>,
    network_config_last_request_ms: Cell<Option<i64>>,
    network_roster_last_request_ms: Cell<Option<i64>>,
}

/// The newest sign-in snapshot among those already queued (spec 0140). Other
/// requests found on the way are kept, in order, for after this one.
fn newest_connect(
    cookie: String,
    requests: &std::sync::mpsc::Receiver<CloudRequest>,
    deferred: &mut std::collections::VecDeque<CloudRequest>,
) -> String {
    let mut cookie = cookie;
    while let Ok(request) = requests.try_recv() {
        match request {
            CloudRequest::Connect(newer) => cookie = newer,
            other => deferred.push_back(other),
        }
    }
    cookie
}

enum CloudRequest {
    Connect(String),
    Refresh,
    Disconnect,
    UpdateRouterConfig(RouterConfigUpdate),
    /// A pause, unpause, or rename from the Network page (spec 0170).
    UpdateClient(RouterConfigUpdate),
    UpdateDishConfig(DishConfigUpdate),
    ReadRouterWifiConfig {
        epoch: u64,
    },
    ReadRouterWifiClients {
        epoch: u64,
    },
}

enum CloudUpdate {
    State(CloudState),
    RouterWriteResult {
        result: Result<(), String>,
        refreshed_config: Option<Result<WifiConfig, String>>,
        requested_subnet: Option<String>,
    },
    ClientWriteResult(Result<(), String>),
    DishWriteResult(Result<(), String>),
    RouterWifiConfig {
        epoch: u64,
        result: Result<serde_json::Value, dishy_cloud::CloudError>,
    },
    RouterWifiClients {
        epoch: u64,
        result: Result<serde_json::Value, dishy_cloud::CloudError>,
    },
}

impl AppState {
    fn new(
        app: adw::Application,
        paths: HostPaths,
        preferences: PreferencesStore,
        translator: Translator,
        hidden_at_startup: bool,
    ) -> Self {
        let (tray_commands, tray_receiver) = mpsc::channel();
        let (settings_actions, settings_receiver) = mpsc::channel();
        let (cloud_actions, cloud_action_receiver) = mpsc::channel();
        let (cloud_requests, cloud_request_receiver) = mpsc::channel();
        let (network_cloud_actions, network_cloud_action_receiver) = mpsc::channel();
        let initial_readout = live_readout(None, now_ms());
        Self {
            app,
            paths,
            preferences: RefCell::new(preferences),
            translator: RefCell::new(translator),
            activation: RefCell::new(ActivationGate::new(hidden_at_startup)),
            started: Cell::new(false),
            hold: RefCell::new(None),
            window: RefCell::new(None),
            toolbar_layout: RefCell::new(None),
            theme_button: RefCell::new(None),
            header_status: RefCell::new(None),
            connectivity_indicator: RefCell::new(None),
            glass: RefCell::new(None),
            status_bus: RefCell::new(None),
            notification_badge: RefCell::new(None),
            dashboard: RefCell::new(None),
            dashboard_samples: RefCell::new(Arc::new(Vec::new())),
            sky: RefCell::new(None),
            data_usage: RefCell::new(None),
            alerts: RefCell::new(None),
            account: RefCell::new(None),
            network: RefCell::new(None),
            speed_test: RefCell::new(None),
            alignment: RefCell::new(None),
            network_cloud_actions,
            network_cloud_action_receiver,
            settings: RefCell::new(None),
            latest_status: RefCell::new(None),
            latest_alerts: RefCell::new(None),
            latest_network: RefCell::new(None),
            obstruction_map: RefCell::new(None),
            last_dashboard_refresh_ms: Cell::new(0),
            tray: RefCell::new(None),
            tray_commands,
            tray_receiver,
            settings_actions,
            settings_receiver,
            recorder: RefCell::new(None),
            recorder_updates: RefCell::new(None),
            latest_sample: Cell::new(None),
            latest_router_reachable: Cell::new(None),
            tray_readout: RefCell::new(initial_readout),
            notifier: RefCell::new(NotificationThrottle::new()),
            chimes: RefCell::new(crate::alert_sound::ChimeThrottle::default()),
            pump_started: Cell::new(false),
            cloud_actions,
            cloud_action_receiver,
            cloud_requests,
            cloud_request_receiver: RefCell::new(Some(cloud_request_receiver)),
            cloud_updates: RefCell::new(None),
            cloud_state: RefCell::new(CloudState::Loading),
            login_window: RefCell::new(None),
            network_cloud_demand: Cell::new(false),
            network_cloud_epoch: Cell::new(0),
            network_lan_reachable: Cell::new(None),
            network_roster_requested: Cell::new(false),
            network_cloud_roster_displayed: Cell::new(false),
            network_config_success_epoch: Cell::new(None),
            network_config_inflight: Cell::new(None),
            network_roster_inflight: Cell::new(None),
            network_config_last_request_ms: Cell::new(None),
            network_roster_last_request_ms: Cell::new(None),
        }
    }

    fn start(self: &Rc<Self>) {
        if self.started.replace(true) {
            return;
        }
        // Keep the process alive even while the only window is hidden and no tray
        // host exists. Dropping this guard on quit is what lets GApplication stop.
        *self.hold.borrow_mut() = Some(self.app.hold());

        self.apply_theme(self.preferences.borrow().get().theme);
        self.start_recorder();
        self.start_cloud_worker();
        if self.preferences.borrow().get().show_tray {
            self.start_tray();
        }
        self.start_status_bus();
        self.start_pump();
    }

    fn activate(self: &Rc<Self>) {
        match self.activation.borrow_mut().activate() {
            Activation::Show => self.show_window(),
            Activation::StayHidden => {}
        }
    }

    fn start_recorder(&self) {
        let data_dir = self.paths.data_dir().to_path_buf();
        let preferences_borrow = self.preferences.borrow();
        let preferences = preferences_borrow.get();
        let router_address = preferences.router_address.clone();
        let router_port = preferences.router_port;
        drop(preferences_borrow);
        match RecorderThread::start_with_router_endpoint(data_dir, router_address, router_port) {
            Ok((thread, updates)) => {
                *self.recorder.borrow_mut() = Some(thread);
                *self.recorder_updates.borrow_mut() = Some(updates);
            }
            Err(error) => tracing::error!("could not start recorder thread: {error}"),
        }
    }

    fn start_cloud_worker(&self) {
        let Some(requests) = self.cloud_request_receiver.borrow_mut().take() else {
            return;
        };
        let (updates, update_receiver) = mpsc::channel();
        *self.cloud_updates.borrow_mut() = Some(update_receiver);
        if let Err(error) = std::thread::Builder::new()
            .name("dishy-cloud".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(error) => {
                        let _ = updates.send(CloudUpdate::State(CloudState::Error(format!(
                            "Cloud runtime unavailable: {error}"
                        ))));
                        return;
                    }
                };
                let client = Arc::new(CloudClient::new(
                    Arc::new(ReqwestTransport::default()),
                    Arc::new(SecretToolSessionStore),
                ));
                let initial_state = match client.is_connected() {
                    Ok(true) => CloudState::Authenticated,
                    Ok(false) => CloudState::SignedOut,
                    Err(error) => CloudState::Error(error.to_string()),
                };
                let _ = updates.send(CloudUpdate::State(initial_state));
                let mut deferred = std::collections::VecDeque::new();
                while let Some(request) = deferred.pop_front().or_else(|| requests.recv().ok()) {
                    match request {
                        CloudRequest::Connect(cookie) => {
                            // The jar is resubmitted as each cookie lands; only
                            // the newest snapshot is worth verifying.
                            let cookie = newest_connect(cookie, &requests, &mut deferred);
                            let _ = updates.send(CloudUpdate::State(CloudState::Loading));
                            let result = runtime.block_on(async {
                                client.connect(&cookie).await?;
                                let _ = updates.send(CloudUpdate::State(CloudState::Authenticated));
                                let (account, usage) =
                                    tokio::try_join!(client.account(), client.usage())?;
                                Ok::<_, dishy_cloud::CloudError>((account, usage))
                            });
                            match &result {
                                Ok(_) => tracing::info!("sign-in: session verified and stored"),
                                Err(error) => {
                                    tracing::warn!("sign-in: verification failed: {error}")
                                }
                            }
                            let state = result
                                .map(|(account, usage)| CloudState::Ready { account, usage })
                                .unwrap_or_else(|error| CloudState::Error(error.to_string()));
                            let _ = updates.send(CloudUpdate::State(state));
                        }
                        CloudRequest::Refresh => {
                            let _ = updates.send(CloudUpdate::State(CloudState::Loading));
                            let result = runtime.block_on(async {
                                let (account, usage) =
                                    tokio::try_join!(client.account(), client.usage())?;
                                Ok::<_, dishy_cloud::CloudError>((account, usage))
                            });
                            let state = result
                                .map(|(account, usage)| CloudState::Ready { account, usage })
                                .unwrap_or_else(|error| {
                                    if error == dishy_cloud::CloudError::NotConnected
                                        || error == dishy_cloud::CloudError::SessionExpired
                                    {
                                        CloudState::SignedOut
                                    } else {
                                        CloudState::Error(error.to_string())
                                    }
                                });
                            let _ = updates.send(CloudUpdate::State(state));
                        }
                        CloudRequest::Disconnect => {
                            let result = client.disconnect();
                            let _ = updates.send(CloudUpdate::State(match result {
                                Ok(()) => CloudState::SignedOut,
                                Err(error) => CloudState::Error(error.to_string()),
                            }));
                        }
                        CloudRequest::UpdateRouterConfig(update) => {
                            let operation = router_config_operation(&update);
                            let requested_subnet = match &update {
                                RouterConfigUpdate::Subnet { subnet, .. } => Some(subnet.clone()),
                                _ => None,
                            };
                            tracing::info!(
                                operation,
                                requested_subnet = requested_subnet.as_deref().unwrap_or("n/a"),
                                "router config write started"
                            );
                            let refresh_config = matches!(
                                &update,
                                RouterConfigUpdate::CustomDns { .. }
                                    | RouterConfigUpdate::Subnet { .. }
                            );
                            let result = runtime
                                .block_on(client.update_router_config(update))
                                .map(|_| ())
                                .map_err(|error| error.to_string());
                            match &result {
                                Ok(()) => tracing::info!(
                                    operation,
                                    "router config write acknowledged by gateway"
                                ),
                                Err(error) => {
                                    tracing::warn!(operation, error, "router config write failed")
                                }
                            }
                            let refreshed_config = if result.is_ok() && refresh_config {
                                tracing::info!(operation, "router config read-back started");
                                let result = runtime
                                    .block_on(async {
                                        let value = client.router_wifi_config().await?;
                                        serde_json::from_value::<WifiConfig>(value).map_err(
                                            |error| {
                                                dishy_cloud::CloudError::GrpcProtocol(
                                                    error.to_string(),
                                                )
                                            },
                                        )
                                    })
                                    .map_err(|error| error.to_string());
                                match &result {
                                    Ok(config) => {
                                        let actual_subnet = config.subnet().unwrap_or("unknown");
                                        let subnet_matches = requested_subnet
                                            .as_deref()
                                            .is_none_or(|requested| requested == actual_subnet);
                                        tracing::info!(
                                            operation,
                                            requested_subnet =
                                                requested_subnet.as_deref().unwrap_or("n/a"),
                                            actual_subnet,
                                            subnet_matches,
                                            "router config read-back completed"
                                        );
                                    }
                                    Err(error) => tracing::warn!(
                                        operation,
                                        error,
                                        "router config read-back failed"
                                    ),
                                }
                                Some(result)
                            } else {
                                None
                            };
                            let _ = updates.send(CloudUpdate::RouterWriteResult {
                                result,
                                refreshed_config,
                                requested_subnet,
                            });
                        }
                        CloudRequest::UpdateClient(update) => {
                            // Sent once; a failure is reported, never retried.
                            let result = runtime
                                .block_on(client.update_router_config(update))
                                .map(|_| ())
                                .map_err(|error| error.to_string());
                            let _ = updates.send(CloudUpdate::ClientWriteResult(result));
                        }
                        CloudRequest::UpdateDishConfig(update) => {
                            let result = runtime
                                .block_on(client.update_dish_config(update))
                                .map(|_| ())
                                .map_err(|error| error.to_string());
                            let _ = updates.send(CloudUpdate::DishWriteResult(result));
                        }
                        CloudRequest::ReadRouterWifiConfig { epoch } => {
                            let result = runtime.block_on(client.router_wifi_config());
                            let _ = updates.send(CloudUpdate::RouterWifiConfig { epoch, result });
                        }
                        CloudRequest::ReadRouterWifiClients { epoch } => {
                            let result = runtime.block_on(client.router_wifi_clients());
                            let _ = updates.send(CloudUpdate::RouterWifiClients { epoch, result });
                        }
                    }
                }
            })
        {
            tracing::error!("could not start cloud worker: {error}");
        }
    }

    /// Publish the live status for the Shell extension's pill (spec 0189).
    fn start_status_bus(&self) {
        let Some(connection) = self.app.dbus_connection() else {
            return;
        };
        match crate::status_bus::StatusBus::register(&connection) {
            Ok(bus) => *self.status_bus.borrow_mut() = Some(bus),
            Err(error) => tracing::warn!("status not published on D-Bus: {error}"),
        }
    }

    /// The link as the header, glass, and status bus show it: whether either
    /// device has answered yet, and whether they are reachable.
    fn link_reachability(&self, dish_override: Option<bool>) -> (Option<bool>, bool) {
        let alerts = self.latest_alerts.borrow();
        let dish_reachable = dish_override
            .or_else(|| alerts.as_ref().and_then(|snapshot| snapshot.dish_reachable))
            .or_else(|| self.latest_status.borrow().as_ref().map(|_| true));
        let router_reachable = self.latest_router_reachable.get().or_else(|| {
            alerts
                .as_ref()
                .and_then(|snapshot| snapshot.router_reachable)
        });
        let online = ui::devices_reachable(dish_reachable, router_reachable);
        let known = dish_reachable.is_some() || router_reachable.is_some();
        (known.then_some(online), online)
    }

    fn publish_status(&self, now_ms: i64) {
        if self.status_bus.borrow().is_none() {
            return;
        }
        let (reachable, _) = self.link_reachability(None);
        let latency = self
            .dashboard_samples
            .borrow()
            .last()
            .and_then(|sample| sample.latency_ms);
        let active = self
            .latest_alerts
            .borrow()
            .as_ref()
            .map_or(0, |snapshot| snapshot.active.len());
        let properties = crate::status_bus::status_properties(
            reachable,
            self.latest_status.borrow().as_ref(),
            self.latest_sample.get().as_ref(),
            latency,
            active,
            now_ms,
            &self.translator.borrow(),
        );
        if let Some(bus) = self.status_bus.borrow().as_ref() {
            bus.publish(properties);
        }
    }

    /// Ask the Shell how it sees the extension, enable it if the user asked
    /// for it and the Shell now knows it, and show the result (spec 0192).
    fn refresh_shell_extension(self: &Rc<Self>) {
        use crate::shell_extension::{
            ExtensionStatus, UUID, is_gnome_session, is_installed, should_enable, status_for,
        };
        let gnome = is_gnome_session();
        let installed = is_installed(&crate::shell_extension::default_extensions_dir());
        let connection = self.app.dbus_connection();
        let state = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let call = |method: &'static str| {
                let connection = connection.clone();
                async move {
                    connection?
                        .call_future(
                            Some("org.gnome.Shell.Extensions"),
                            "/org/gnome/Shell/Extensions",
                            "org.gnome.Shell.Extensions",
                            method,
                            Some(&(UUID,).to_variant()),
                            None,
                            gio::DBusCallFlags::NONE,
                            5_000,
                        )
                        .await
                        .ok()
                }
            };
            let shell_state = if gnome {
                call("GetExtensionInfo").await.and_then(|reply| {
                    glib::VariantDict::new(Some(&reply.child_value(0)))
                        .lookup::<f64>("state")
                        .ok()
                        .flatten()
                })
            } else {
                None
            };
            let mut status = status_for(gnome, installed, shell_state);
            let Some(state) = state.upgrade() else { return };
            let requested = state.preferences.borrow().get().shell_extension_requested;
            if should_enable(requested, status) {
                let enabled = call("EnableExtension")
                    .await
                    .and_then(|reply| reply.child_value(0).get::<bool>())
                    .unwrap_or(false);
                if enabled {
                    status = ExtensionStatus::Enabled;
                }
            }
            // Once on, the request is done: switching it off later in GNOME
            // Extensions is the user's call and is left alone.
            if requested
                && status == ExtensionStatus::Enabled
                && let Err(error) = state
                    .preferences
                    .borrow_mut()
                    .set(|preferences| preferences.shell_extension_requested = false)
            {
                tracing::warn!("could not clear the extension request: {error}");
            }
            if let Some(settings) = state.settings.borrow().as_ref() {
                settings.set_shell_extension_status(status);
            }
        });
    }

    fn set_show_tray(&self, on: bool) {
        if let Err(error) = self
            .preferences
            .borrow_mut()
            .set(|preferences| preferences.show_tray = on)
        {
            tracing::warn!("could not save tray preference: {error}");
        }
        if on {
            if self.tray.borrow().is_none() {
                self.start_tray();
                self.refresh_tray(None);
            }
        } else if let Some(tray) = self.tray.borrow_mut().take() {
            let _ = tray.shutdown();
        }
    }

    fn start_tray(&self) {
        let state = self.tray_state();
        let translator = self.translator.borrow().clone();
        let handle = crate::tray::start(self.tray_commands.clone(), translator, state);
        *self.tray.borrow_mut() = handle;
    }

    fn start_pump(self: &Rc<Self>) {
        if self.pump_started.replace(true) {
            return;
        }
        let state = self.clone();
        glib::timeout_add_local(Duration::from_millis(250), move || {
            state.pump();
            // Signals only what changed since the last tick (spec 0189).
            state.publish_status(now_ms());
            glib::ControlFlow::Continue
        });
    }

    fn pump(self: &Rc<Self>) {
        while let Ok(action) = self.cloud_action_receiver.try_recv() {
            self.handle_cloud_action(action);
        }
        while let Ok(action) = self.network_cloud_action_receiver.try_recv() {
            match action {
                NetworkCloudAction::Demand(wanted) => {
                    if self.network_cloud_demand.replace(wanted) != wanted {
                        self.network_cloud_epoch
                            .set(self.network_cloud_epoch.get().wrapping_add(1));
                        self.network_config_success_epoch.set(None);
                        self.network_config_inflight.set(None);
                        self.network_roster_inflight.set(None);
                        self.network_config_last_request_ms.set(None);
                        self.network_roster_last_request_ms.set(None);
                    }
                }
                NetworkCloudAction::Pause {
                    client_id,
                    paused,
                    mac_address,
                } => {
                    let _ = self.cloud_requests.send(CloudRequest::UpdateClient(
                        RouterConfigUpdate::ClientPause {
                            client_id,
                            paused,
                            mac_address,
                        },
                    ));
                }
                NetworkCloudAction::Rename {
                    client_id,
                    given_name,
                } => {
                    let _ = self.cloud_requests.send(CloudRequest::UpdateClient(
                        RouterConfigUpdate::ClientRename {
                            client_id,
                            given_name,
                        },
                    ));
                }
                NetworkCloudAction::RequestRoster => {
                    if self.network_cloud_demand.get()
                        && self.network_lan_reachable.get() == Some(false)
                    {
                        self.network_roster_requested.set(true);
                        self.network_roster_last_request_ms.set(None);
                    }
                }
            }
        }
        loop {
            let update = self
                .cloud_updates
                .borrow()
                .as_ref()
                .and_then(|rx| rx.try_recv().ok());
            match update {
                Some(CloudUpdate::State(state)) => {
                    // Take the window out before closing it: its close handler
                    // clears the same cell and must not find it borrowed.
                    let login =
                        matches!(&state, CloudState::Authenticated | CloudState::Ready { .. })
                            .then(|| self.login_window.borrow_mut().take())
                            .flatten();
                    if let Some(login) = login {
                        login.close();
                    }
                    self.cloud_state.replace(state.clone());
                    if let Some(network) = self.network.borrow().as_ref() {
                        network.set_cloud_connected(matches!(
                            &state,
                            CloudState::Authenticated | CloudState::Ready { .. }
                        ));
                    }
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.set_account_connected(matches!(
                            &state,
                            CloudState::Authenticated | CloudState::Ready { .. }
                        ));
                        match &state {
                            CloudState::Ready { account, .. } => {
                                let bypassed = account
                                    .device_telemetry
                                    .as_object()
                                    .and_then(|devices| {
                                        devices.values().find(|device| {
                                            device.get("kind").and_then(serde_json::Value::as_str)
                                                == Some("router")
                                                && device
                                                    .get("hops")
                                                    .and_then(serde_json::Value::as_f64)
                                                    == Some(0.0)
                                        })
                                    })
                                    .and_then(|device| {
                                        device
                                            .get("isBypassed")
                                            .and_then(serde_json::Value::as_bool)
                                    });
                                settings.set_bypass_state(bypassed);
                            }
                            CloudState::SignedOut => settings.set_bypass_state(None),
                            _ => {}
                        }
                    }
                    if let Some(page) = self.account.borrow().as_ref() {
                        page.set_state(state);
                    }
                    if matches!(
                        &*self.cloud_state.borrow(),
                        CloudState::Authenticated | CloudState::Ready { .. }
                    ) && let Some(page) = self.network.borrow().as_ref()
                    {
                        page.set_cloud_roster_state(false, None);
                    }
                }
                Some(CloudUpdate::RouterWriteResult {
                    result,
                    refreshed_config,
                    requested_subnet,
                }) => {
                    let subnet_mismatch = requested_subnet.as_deref().and_then(|requested| {
                        refreshed_config
                            .as_ref()
                            .and_then(|result| result.as_ref().ok())
                            .and_then(|config| router_subnet_mismatch(requested, config))
                    });
                    if let Some(Ok(config)) = refreshed_config.as_ref() {
                        self.apply_cloud_network_config(config.clone());
                    }
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        match result {
                            Err(error) => settings.set_cloud_write_result(Err(error)),
                            Ok(()) => match (subnet_mismatch, refreshed_config) {
                                (Some((requested, actual)), _) => {
                                    settings.set_config_not_applied(&requested, &actual);
                                }
                                (None, Some(Err(error))) => {
                                    tracing::warn!(
                                        "router settings write succeeded but refresh failed: {error}"
                                    );
                                    settings.set_config_refresh_error(&error);
                                }
                                _ => settings.set_cloud_write_result(Ok(())),
                            },
                        }
                    }
                }
                Some(CloudUpdate::ClientWriteResult(result)) => {
                    if let Some(network) = self.network.borrow().as_ref() {
                        network.client_write_result(result);
                    }
                }
                Some(CloudUpdate::DishWriteResult(result)) => {
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.set_dish_write_result(result);
                    }
                }
                Some(CloudUpdate::RouterWifiConfig { epoch, result }) => {
                    if self.network_config_inflight.get() == Some(epoch) {
                        self.network_config_inflight.set(None);
                    }
                    if epoch == self.network_cloud_epoch.get()
                        && self.network_cloud_demand.get()
                        && self.network_lan_reachable.get() == Some(false)
                    {
                        match result {
                            Ok(value) => match serde_json::from_value::<WifiConfig>(value) {
                                Ok(config) => {
                                    self.network_config_success_epoch.set(Some(epoch));
                                    self.apply_cloud_network_config(config);
                                }
                                Err(error) => {
                                    tracing::warn!(
                                        "cloud router Wi-Fi config decode failed: {error}"
                                    );
                                    self.show_cloud_network_error(
                                        &dishy_cloud::CloudError::GrpcProtocol(error.to_string()),
                                    );
                                }
                            },
                            Err(error) => {
                                tracing::warn!("cloud router Wi-Fi config read failed: {error}");
                                self.show_cloud_network_error(&error);
                            }
                        }
                    }
                }
                Some(CloudUpdate::RouterWifiClients { epoch, result }) => {
                    if self.network_roster_inflight.get() == Some(epoch) {
                        self.network_roster_inflight.set(None);
                    }
                    if epoch == self.network_cloud_epoch.get()
                        && self.network_cloud_demand.get()
                        && self.network_lan_reachable.get() == Some(false)
                    {
                        match result {
                            Ok(value) => match serde_json::from_value::<Vec<WifiClient>>(value) {
                                Ok(clients) => self.apply_cloud_network_roster(clients),
                                Err(error) => {
                                    tracing::warn!(
                                        "cloud router Wi-Fi roster decode failed: {error}"
                                    );
                                    self.show_cloud_network_error(
                                        &dishy_cloud::CloudError::GrpcProtocol(error.to_string()),
                                    );
                                }
                            },
                            Err(error) => {
                                tracing::warn!("cloud router Wi-Fi roster read failed: {error}");
                                self.show_cloud_network_error(&error);
                            }
                        }
                    }
                }
                None => break,
            }
        }
        loop {
            let command = self.tray_receiver.try_recv().ok();
            match command {
                Some(command) => self.handle_tray(command),
                None => break,
            }
        }
        loop {
            let action = self.settings_receiver.try_recv().ok();
            match action {
                Some(action) => self.handle_settings(action),
                None => break,
            }
        }

        let mut got_update = false;
        loop {
            let update = {
                let receiver = self.recorder_updates.borrow();
                receiver
                    .as_ref()
                    .and_then(|receiver| receiver.try_recv().ok())
            };
            match update {
                Some(RecorderUpdate::Sample(sample)) => {
                    self.latest_sample.set(Some(sample));
                    got_update = true;
                }
                Some(RecorderUpdate::Samples(samples)) => {
                    if let Some(speed_test) = self.speed_test.borrow().as_ref() {
                        speed_test.set_samples(samples.clone());
                    }
                    *self.dashboard_samples.borrow_mut() = samples;
                    got_update = true;
                }
                Some(RecorderUpdate::Status(status)) => {
                    if let Some(speed_test) = self.speed_test.borrow().as_ref() {
                        speed_test.set_dish_model(&crate::sky_view::dish_model_for(Some(&status)));
                    }
                    if let Some(account) = self.account.borrow().as_ref() {
                        // Devices answering here read live on the account page.
                        let mut lan: std::collections::HashSet<String> =
                            status.connected_routers.iter().cloned().collect();
                        if let Some(id) =
                            status.device_info.as_ref().and_then(|info| info.id.clone())
                        {
                            lan.insert(id);
                        }
                        account.set_lan_online(lan);
                    }
                    let status = (*status).clone();
                    if let Some(sky) = self.sky.borrow().as_ref() {
                        sky.set_status(Some(status.clone()));
                    }
                    if let Some(alignment) = self.alignment.borrow().as_ref() {
                        alignment.set_status(Some(status.clone()));
                    }
                    self.latest_status.replace(Some(status));
                    self.refresh_header_status(Some(true));
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.set_status(self.latest_status.borrow().clone());
                    }
                    got_update = true;
                }
                Some(RecorderUpdate::Obstruction(map)) => {
                    let map = (*map).clone();
                    if let Some(sky) = self.sky.borrow().as_ref() {
                        sky.set_obstruction(Some(map.clone()));
                    }
                    if let Some(dashboard) = self.dashboard.borrow().as_ref() {
                        dashboard.set_obstruction(Some(map.clone()));
                    }
                    self.obstruction_map.replace(Some(map));
                    got_update = true;
                }
                Some(RecorderUpdate::Usage(summary)) => {
                    if let Some(page) = self.data_usage.borrow().as_ref() {
                        page.set_summary((*summary).clone());
                    }
                }
                Some(RecorderUpdate::ClientTotals(totals)) => {
                    if let Some(network) = self.network.borrow().as_ref() {
                        network.set_known_totals(
                            totals
                                .iter()
                                .map(|total| {
                                    (
                                        dishy_core::odometer::key_of(
                                            total.client_id,
                                            &total.mac_address,
                                        ),
                                        (total.rx_bytes, total.tx_bytes),
                                    )
                                })
                                .collect(),
                        );
                        network.set_known_devices(
                            totals
                                .iter()
                                .filter_map(|total| {
                                    let name = total.name.clone()?;
                                    let key = dishy_core::odometer::key_of(
                                        total.client_id,
                                        &total.mac_address,
                                    );
                                    Some((key, name))
                                })
                                .collect(),
                        );
                    }
                    if let Some(page) = self.data_usage.borrow().as_ref() {
                        page.set_client_totals((*totals).clone());
                    }
                }
                Some(RecorderUpdate::ClientMonthlyLimits(limits)) => {
                    if let Some(page) = self.data_usage.borrow().as_ref() {
                        page.set_client_monthly_limits((*limits).clone());
                    }
                }
                Some(RecorderUpdate::ClientMonthlyLimitsChanged(limits)) => {
                    if let Some(page) = self.data_usage.borrow().as_ref() {
                        page.set_client_monthly_limits((*limits).clone());
                        page.set_monthly_limit_error(None);
                    }
                }
                Some(RecorderUpdate::DeviceThroughput { key, samples }) => {
                    if let Some(network) = self.network.borrow().as_ref() {
                        network.set_device_throughput(&key, (*samples).clone());
                    }
                }
                Some(RecorderUpdate::ObstructionSnapshots(snapshots)) => {
                    if let Some(sky) = self.sky.borrow().as_ref() {
                        sky.set_obstruction_snapshots((*snapshots).clone());
                    }
                }
                Some(RecorderUpdate::MergeCandidates(candidates)) => {
                    if let Some(page) = self.data_usage.borrow().as_ref() {
                        page.set_merge_candidates((*candidates).clone());
                    }
                }
                Some(RecorderUpdate::Events(snapshot)) => {
                    if let Some(alerts) = self.alerts.borrow().as_ref() {
                        alerts.set_events(&snapshot);
                    }
                    if let Some(dashboard) = self.dashboard.borrow().as_ref() {
                        dashboard.set_events(&snapshot);
                    }
                }
                Some(RecorderUpdate::DetailEnergy(summary)) => {
                    if let Some(dashboard) = self.dashboard.borrow().as_ref() {
                        dashboard.deliver_detail_energy(summary);
                    }
                }
                Some(RecorderUpdate::LatencySummary(summary)) => {
                    if let Some(dashboard) = self.dashboard.borrow().as_ref() {
                        dashboard.deliver_latency_summary(summary);
                    }
                }
                Some(RecorderUpdate::Rules(rules)) => {
                    if let Some(network) = self.network.borrow().as_ref() {
                        network.set_rules(rules);
                    }
                }
                Some(RecorderUpdate::RuleWriteFailed(error)) => {
                    if let Some(network) = self.network.borrow().as_ref() {
                        network.rule_write_failed(&error);
                    }
                }
                Some(RecorderUpdate::MonthlyLimitWriteFailed(error)) => {
                    if let Some(page) = self.data_usage.borrow().as_ref() {
                        page.set_monthly_limit_error(Some(error));
                    }
                }
                Some(RecorderUpdate::Alerts(alerts)) => {
                    self.handle_alert_snapshot((*alerts).clone());
                    got_update = true;
                }
                Some(RecorderUpdate::Network(network)) => {
                    let snapshot = (*network).clone();
                    let was_reachable =
                        self.network_lan_reachable.replace(Some(snapshot.reachable));
                    if was_reachable != Some(snapshot.reachable) {
                        self.network_cloud_epoch
                            .set(self.network_cloud_epoch.get().wrapping_add(1));
                        if snapshot.reachable {
                            self.network_roster_requested.set(false);
                        }
                        self.network_config_success_epoch.set(None);
                        self.network_config_inflight.set(None);
                        self.network_roster_inflight.set(None);
                        self.network_config_last_request_ms.set(None);
                        self.network_roster_last_request_ms.set(None);
                        self.network_cloud_roster_displayed.set(false);
                    }
                    self.latest_network.replace(Some(network.clone()));
                    if let Some(page) = self.network.borrow().as_ref() {
                        page.set_snapshot(snapshot.clone());
                    }
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.set_network_snapshot(Some(&snapshot));
                    }
                }
                Some(RecorderUpdate::DishConfig(config)) => {
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.set_dish_config(&config);
                    }
                    got_update = true;
                }
                Some(RecorderUpdate::RouterReachable(reachable)) => {
                    self.latest_router_reachable.set(Some(reachable));
                    self.refresh_header_status(None);
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.set_router_reachable(Some(reachable));
                    }
                }
                Some(RecorderUpdate::Control(report)) => {
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.set_control_result(report);
                    }
                }
                Some(RecorderUpdate::Diagnostics(result)) => {
                    if let Some(settings) = self.settings.borrow().as_ref() {
                        settings.set_diagnostics_result(result);
                    }
                }
                Some(RecorderUpdate::Failed(error)) => {
                    tracing::warn!("recorder unavailable: {error}");
                }
                None => break,
            }
        }

        self.pump_network_cloud();

        let readout = live_readout(self.latest_sample.get().as_ref(), now_ms());
        let changed = readout != *self.tray_readout.borrow();
        if changed {
            *self.tray_readout.borrow_mut() = readout;
        }
        if got_update || changed {
            self.refresh_tray(None);
        }
        self.refresh_dashboard_if_due(got_update);
    }

    fn pump_network_cloud(&self) {
        // SPEC-NETCLOUD-001..004: account reads require visible Network,
        // confirmed LAN failure, and a live account session.
        if !self.network_cloud_demand.get() || self.network_lan_reachable.get() != Some(false) {
            return;
        }
        let connected = matches!(
            &*self.cloud_state.borrow(),
            CloudState::Authenticated | CloudState::Ready { .. }
        );
        if !connected {
            let message = self.translator.borrow().text("errors.connectAccountFirst");
            if let Some(page) = self.network.borrow().as_ref() {
                page.set_cloud_roster_state(false, Some(message));
            }
            return;
        }
        if !cloud_network_reads_allowed(
            self.network_cloud_demand.get(),
            self.network_lan_reachable.get(),
            connected,
        ) {
            return;
        }

        let now = now_ms();
        let epoch = self.network_cloud_epoch.get();
        let due = |last: Option<i64>| {
            last.is_none_or(|last| now.saturating_sub(last) >= CLOUD_NETWORK_POLL_INTERVAL_MS)
        };
        if self.network_config_success_epoch.get() != Some(epoch)
            && self.network_config_inflight.get() != Some(epoch)
            && due(self.network_config_last_request_ms.get())
            && self
                .cloud_requests
                .send(CloudRequest::ReadRouterWifiConfig { epoch })
                .is_ok()
        {
            self.network_config_inflight.set(Some(epoch));
            self.network_config_last_request_ms.set(Some(now));
        }
        if cloud_roster_reads_allowed(
            self.network_cloud_demand.get(),
            self.network_lan_reachable.get(),
            connected,
            self.network_roster_requested.get(),
        ) && self.network_roster_inflight.get() != Some(epoch)
            && due(self.network_roster_last_request_ms.get())
            && self
                .cloud_requests
                .send(CloudRequest::ReadRouterWifiClients { epoch })
                .is_ok()
        {
            self.network_roster_inflight.set(Some(epoch));
            self.network_roster_last_request_ms.set(Some(now));
            if let Some(page) = self.network.borrow().as_ref() {
                page.set_cloud_roster_state(true, None);
            }
        }
    }

    fn apply_cloud_network_config(&self, config: WifiConfig) {
        let mut snapshot =
            self.latest_network
                .borrow()
                .as_deref()
                .cloned()
                .unwrap_or(NetworkSnapshot {
                    reachable: false,
                    devices: Vec::new(),
                    nodes: Vec::new(),
                    config: None,
                    radios: Vec::new(),
                });
        for device in &mut snapshot.devices {
            if device.name.is_none() {
                device.name = config
                    .saved_name_for(device.client_id, Some(device.mac_address.as_str()))
                    .map(str::to_owned)
                    .or_else(|| device.hostname.clone());
            }
        }
        snapshot.nodes = build_nodes(&snapshot.devices, Some(&config));
        snapshot.config = Some(config);
        let cloud_roster = self.network_cloud_roster_displayed.get();
        self.publish_network_snapshot(snapshot, cloud_roster);
        if let Some(page) = self.network.borrow().as_ref() {
            page.set_cloud_roster_state(false, None);
        }
    }

    fn apply_cloud_network_roster(&self, clients: Vec<WifiClient>) {
        let config = self
            .latest_network
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.config.clone());
        let mut tracker = ClientRateTracker::default();
        let snapshot = NetworkSnapshot::from_roster(
            &clients,
            config.as_ref(),
            Vec::new(),
            &mut tracker,
            now_ms(),
            true,
        );
        self.network_cloud_roster_displayed.set(true);
        self.publish_network_snapshot(snapshot, true);
        if let Some(page) = self.network.borrow().as_ref() {
            page.set_cloud_roster_state(false, None);
        }
    }

    fn publish_network_snapshot(&self, snapshot: NetworkSnapshot, cloud_roster: bool) {
        let snapshot = Arc::new(snapshot);
        self.latest_network.replace(Some(snapshot.clone()));
        if let Some(page) = self.network.borrow().as_ref() {
            if cloud_roster {
                page.set_cloud_snapshot((*snapshot).clone());
            } else {
                page.set_snapshot((*snapshot).clone());
            }
        }
        if let Some(settings) = self.settings.borrow().as_ref() {
            settings.set_network_snapshot(Some(&snapshot));
        }
    }

    fn show_cloud_network_error(&self, error: &dishy_cloud::CloudError) {
        let needs_sign_in = matches!(
            error,
            dishy_cloud::CloudError::NotConnected | dishy_cloud::CloudError::SessionExpired
        );
        let translator = self.translator.borrow();
        let message = translator.text(if needs_sign_in {
            "errors.connectAccountFirst"
        } else {
            "errors.accountUnreachableCheckInternet"
        });
        if let Some(page) = self.network.borrow().as_ref() {
            page.set_cloud_roster_state(false, Some(message));
        }
        if needs_sign_in {
            self.cloud_state.replace(CloudState::SignedOut);
            if let Some(page) = self.account.borrow().as_ref() {
                page.set_state(CloudState::SignedOut);
            }
            if let Some(settings) = self.settings.borrow().as_ref() {
                settings.set_account_connected(false);
                settings.set_bypass_state(None);
            }
        }
    }

    fn handle_cloud_action(self: &Rc<Self>, action: CloudAction) {
        match action {
            CloudAction::SignIn => {
                let Some(parent) = self.window.borrow().as_ref().cloned() else {
                    return;
                };
                let sender = self.cloud_requests.clone();
                let title = self.translator.borrow().text("desktop.signInTitle");
                let close = self.translator.borrow().text("common.close");
                let login = show_sign_in(&parent, &title, &close, move |cookie| {
                    let _ = sender.send(CloudRequest::Connect(cookie));
                });
                let weak_state = Rc::downgrade(self);
                login.connect_close_request(move |_| {
                    if let Some(state) = weak_state.upgrade() {
                        state.login_window.borrow_mut().take();
                    }
                    glib::Propagation::Proceed
                });
                *self.login_window.borrow_mut() = Some(login);
            }
            CloudAction::Refresh => {
                let _ = self.cloud_requests.send(CloudRequest::Refresh);
            }
            CloudAction::Disconnect => {
                let _ = self.cloud_requests.send(CloudRequest::Disconnect);
            }
        }
    }

    fn refresh_dashboard_if_due(&self, force: bool) {
        let visible = self
            .window
            .borrow()
            .as_ref()
            .is_some_and(|window| window.is_visible());
        if !visible {
            return;
        }
        let now = now_ms();
        if !force && now - self.last_dashboard_refresh_ms.get() < 1_000 {
            return;
        }
        self.last_dashboard_refresh_ms.set(now);
        self.refresh_dashboard(now);
    }

    fn handle_alert_snapshot(&self, snapshot: AlertSnapshot) {
        let now = now_ms();
        let translator = self.translator.borrow();
        if let Some(page) = self.alerts.borrow().as_ref() {
            page.set_snapshot(&snapshot, &translator, now);
        }
        if let Some(sky) = self.sky.borrow().as_ref() {
            sky.set_stale(snapshot.dish_reachable == Some(false));
        }
        for transition in &snapshot.transitions {
            // The chime is its own channel: it plays with notifications off.
            if self.preferences.borrow().get().alert_sounds
                && self
                    .chimes
                    .borrow_mut()
                    .allow(&transition.notification_key(), now)
            {
                crate::alert_sound::play(crate::alert_sound::tones(
                    transition.severity,
                    transition.kind == AlertTransitionKind::Cleared,
                ));
            }
            let enabled = self.preferences.borrow().get().notifications;
            if should_notify_alert(enabled, transition) {
                let device = match transition.source {
                    AlertSource::Dish => translator.text("alerts.deviceDish"),
                    AlertSource::Router => translator.text("alerts.deviceRouter"),
                    AlertSource::System => translator.text("alerts.deviceSystem"),
                };
                let title_key = match transition.kind {
                    AlertTransitionKind::Fired => "alerts.firingTitle",
                    AlertTransitionKind::Cleared => "alerts.clearedTitle",
                };
                let title = translator.text_with(title_key, &[("device", &device)]);
                let body = crate::alerts_view::alert_message(
                    transition.source,
                    &transition.key,
                    transition.label.as_deref(),
                    transition.kind,
                    &translator,
                );
                let key = transition.notification_key();
                self.post_notification(&key, &title, &body);
            }
        }
        let reachable = snapshot.dish_reachable;
        let active_count = snapshot.active.len();
        self.latest_alerts.replace(Some(snapshot));
        self.refresh_header_status(reachable);
        if let Some(badge) = self.notification_badge.borrow().as_ref() {
            if let Some(text) = ui::notification_badge_text(active_count) {
                badge.set_label(&text);
                badge.set_visible(true);
            } else {
                badge.set_visible(false);
            }
        }
    }

    fn refresh_header_status(&self, reachable: Option<bool>) {
        let Some(label) = self.header_status.borrow().as_ref().cloned() else {
            return;
        };
        let (known, online) = self.link_reachability(reachable);
        let translator = self.translator.borrow();
        let status = self.latest_status.borrow();
        let alerts = self.latest_alerts.borrow();
        // The glass glow follows the link (spec 0190).
        let alerting = alerts.as_ref().is_some_and(|snapshot| {
            snapshot.active.iter().any(|alert| {
                matches!(
                    alert.severity,
                    dishy_core::alert::Severity::Critical | dishy_core::alert::Severity::Warning
                )
            })
        });
        if let Some(glass) = self.glass.borrow().as_ref() {
            glass.set_mood(crate::theme::mood_for(online, alerting));
        }
        // "connecting" until either device has answered or failed to.
        label.set_label(&ui::header_status_text(status.as_ref(), known, &translator));
        if let Some(indicator) = self.connectivity_indicator.borrow().as_ref() {
            ui::set_status_indicator(
                indicator,
                &translator.text("desktop.appName"),
                online,
                &translator,
            );
        }
    }

    fn refresh_dashboard(&self, now_ms: i64) {
        let dashboard = self.dashboard.borrow().clone();
        if let Some(dashboard) = dashboard {
            dashboard.set_samples(self.dashboard_samples.borrow().clone());
            dashboard.set_status(self.latest_status.borrow().clone());
            dashboard.refresh(now_ms);
        }
    }

    fn handle_tray(self: &Rc<Self>, command: TrayCommand) {
        match command {
            TrayCommand::Open => self.show_window(),
            TrayCommand::Quit => self.quit(),
            TrayCommand::SetNotifications(on) => {
                self.set_notifications(on);
                self.refresh_tray(None);
            }
            TrayCommand::SetAutostart(on) => {
                self.set_autostart(on);
                self.refresh_tray(None);
            }
        }
    }

    fn handle_settings(self: &Rc<Self>, action: SettingsAction) {
        match action {
            SettingsAction::SetNotifications(on) => {
                self.set_notifications(on);
                self.refresh_tray(None);
                self.refresh_settings_state();
            }
            SettingsAction::SetAutostart(on) => {
                self.set_autostart(on);
                self.refresh_tray(None);
                self.refresh_settings_state();
            }
            SettingsAction::InstallShellExtension => {
                let extensions = crate::shell_extension::default_extensions_dir();
                match crate::shell_extension::install(&extensions) {
                    Ok(path) => {
                        tracing::info!("Shell extension written to {}", path.display());
                        if let Err(error) = self
                            .preferences
                            .borrow_mut()
                            .set(|preferences| preferences.shell_extension_requested = true)
                        {
                            tracing::warn!("could not save the extension request: {error}");
                        }
                    }
                    Err(error) => tracing::warn!("could not install the Shell extension: {error}"),
                }
                self.refresh_shell_extension();
            }
            SettingsAction::SetShowTray(on) => {
                self.set_show_tray(on);
                self.refresh_settings_state();
            }
            SettingsAction::SetSeeThroughGlass(on) => {
                if let Err(error) = self
                    .preferences
                    .borrow_mut()
                    .set(|preferences| preferences.see_through_glass = on)
                {
                    tracing::warn!("could not save see-through glass: {error}");
                }
                if let Some(glass) = self.glass.borrow().as_ref() {
                    glass.set_see_through(on);
                }
                self.refresh_settings_state();
            }
            SettingsAction::SetTrayThroughput(on) => {
                let mut preferences = self.preferences.borrow_mut();
                if let Err(error) = preferences.set(|preferences| {
                    preferences.menu_bar_throughput = on;
                }) {
                    tracing::warn!("could not save tray throughput preference: {error}");
                }
                drop(preferences);
                self.refresh_tray(None);
                self.refresh_settings_state();
            }
            SettingsAction::SetRouterAddress(address) => {
                let address = address
                    .as_deref()
                    .and_then(crate::preferences::normalize_ip_address);
                let mut preferences = self.preferences.borrow_mut();
                if let Err(error) = preferences.set(|preferences| {
                    preferences.router_address = address.clone();
                }) {
                    tracing::warn!("could not save router address preference: {error}");
                }
                if let Some(recorder) = self.recorder.borrow().as_ref()
                    && recorder
                        .command_sender()
                        .send(RecorderCommand::SetRouterAddress(address))
                        .is_err()
                {
                    tracing::warn!("could not apply router address to recorder");
                }
                drop(preferences);
                self.refresh_settings_state();
            }
            SettingsAction::SetRouterPort(port) => {
                let port = port.filter(|port| (1..=65535).contains(port));
                let mut preferences = self.preferences.borrow_mut();
                if let Err(error) = preferences.set(|preferences| {
                    preferences.router_port = port;
                }) {
                    tracing::warn!("could not save router port preference: {error}");
                }
                if let Some(recorder) = self.recorder.borrow().as_ref()
                    && recorder
                        .command_sender()
                        .send(RecorderCommand::SetRouterPort(port))
                        .is_err()
                {
                    tracing::warn!("could not apply router port to recorder");
                }
                drop(preferences);
                self.refresh_settings_state();
            }
            SettingsAction::UpdateRouterConfig(update) => {
                if self
                    .cloud_requests
                    .send(CloudRequest::UpdateRouterConfig(update))
                    .is_err()
                    && let Some(settings) = self.settings.borrow().as_ref()
                {
                    settings.set_cloud_write_result(Err(self
                        .translator
                        .borrow()
                        .text("errors.cloudNotStarted")));
                }
            }
            SettingsAction::UpdateDishConfig(update) => {
                if self
                    .cloud_requests
                    .send(CloudRequest::UpdateDishConfig(update))
                    .is_err()
                    && let Some(settings) = self.settings.borrow().as_ref()
                {
                    settings.set_dish_write_result(Err(self
                        .translator
                        .borrow()
                        .text("errors.cloudNotStarted")));
                }
            }
            SettingsAction::SetLocale(locale) => self.set_locale(locale),
            SettingsAction::SetTimeFormat(time_format) => {
                let mut preferences = self.preferences.borrow_mut();
                if let Err(error) = preferences.set(|preferences| {
                    preferences.time_format = time_format;
                }) {
                    tracing::warn!("could not save time format preference: {error}");
                }
                drop(preferences);
                self.refresh_settings_state();
            }
            SettingsAction::SetToolbarStyle(toolbar_style) => {
                let mut preferences = self.preferences.borrow_mut();
                if let Err(error) = preferences.set(|preferences| {
                    preferences.toolbar_style = toolbar_style;
                }) {
                    tracing::warn!("could not save toolbar style preference: {error}");
                }
                drop(preferences);
                if let Some(layout) = self.toolbar_layout.borrow().as_ref() {
                    layout.set_style(toolbar_style);
                }
                self.refresh_settings_state();
            }
            SettingsAction::SetSelfClientId(client_id) => {
                let mut preferences = self.preferences.borrow_mut();
                if let Err(error) = preferences.set(|preferences| {
                    preferences.self_client_id = client_id;
                }) {
                    tracing::warn!("could not save self-device preference: {error}");
                }
                drop(preferences);
                if let Some(network) = self.network.borrow().as_ref() {
                    network.set_self_client_id(client_id);
                }
                self.refresh_settings_state();
            }
        }
    }

    fn set_locale(self: &Rc<Self>, locale: Locale) {
        if self.translator.borrow().locale() == locale {
            return;
        }
        let mut preferences = self.preferences.borrow_mut();
        if let Err(error) = preferences.set(|preferences| preferences.locale = Some(locale)) {
            tracing::warn!("could not save locale: {error}");
        }
        drop(preferences);
        self.rebuild_window(locale);
    }

    /// Replace the window so every catalog lookup, including the tray menu, uses
    /// the new locale. The recorder and its history are untouched.
    fn rebuild_window(self: &Rc<Self>, locale: Locale) {
        if let Some(window) = self.window.borrow_mut().take() {
            self.save_bounds(&window);
            window.destroy();
        }
        self.theme_button.borrow_mut().take();
        self.header_status.borrow_mut().take();
        self.connectivity_indicator.borrow_mut().take();
        self.notification_badge.borrow_mut().take();
        self.toolbar_layout.borrow_mut().take();
        self.dashboard.borrow_mut().take();
        self.sky.borrow_mut().take();
        self.data_usage.borrow_mut().take();
        self.alerts.borrow_mut().take();
        self.account.borrow_mut().take();
        self.network.borrow_mut().take();
        self.speed_test.borrow_mut().take();
        self.alignment.borrow_mut().take();
        self.settings.borrow_mut().take();
        *self.translator.borrow_mut() = Translator::new(locale);
        let translator = self.translator.borrow().clone();
        self.refresh_tray(Some(translator));
        self.show_window();
    }

    fn settings_state(&self) -> AppSettingsState {
        let preferences = self.preferences.borrow();
        AppSettingsState {
            locale: self.translator.borrow().locale(),
            notifications: notifications_requested(preferences.get().notifications),
            autostart: Autostart::new(self.paths.autostart_file()).is_installed(),
            tray_throughput: preferences.get().menu_bar_throughput,
            see_through_glass: preferences.get().see_through_glass,
            show_tray: preferences.get().show_tray,
            router_address: preferences.get().router_address.clone(),
            router_port: preferences.get().router_port,
            time_format: preferences.get().time_format,
            toolbar_style: preferences.get().toolbar_style,
            self_client_id: preferences.get().self_client_id,
        }
    }

    fn refresh_settings_state(&self) {
        if let Some(settings) = self.settings.borrow().as_ref() {
            settings.set_app_state(self.settings_state());
        }
    }

    fn show_window(self: &Rc<Self>) {
        if self.window.borrow().is_none() {
            let theme = self.preferences.borrow().get().theme;
            let bounds = self
                .preferences
                .borrow()
                .get()
                .window_bounds
                .and_then(|saved| clamp_bounds(saved, &connected_displays()));
            let translator = self.translator.borrow().clone();
            let toolbar_style = self.preferences.borrow().get().toolbar_style;
            let shell = ui::build_window(&self.app, &translator, theme, bounds, toolbar_style);
            *self.theme_button.borrow_mut() = Some(shell.theme_button.clone());
            *self.header_status.borrow_mut() = Some(shell.header_status.clone());
            *self.connectivity_indicator.borrow_mut() = Some(shell.connectivity_indicator.clone());
            shell
                .glass
                .set_see_through(self.preferences.borrow().get().see_through_glass);
            *self.glass.borrow_mut() = Some(shell.glass.clone());
            *self.notification_badge.borrow_mut() = Some(shell.notification_badge.clone());
            *self.toolbar_layout.borrow_mut() = Some(shell.toolbar_layout.clone());
            *self.dashboard.borrow_mut() = Some(shell.dashboard.clone());
            *self.sky.borrow_mut() = Some(shell.sky.clone());
            *self.data_usage.borrow_mut() = Some(shell.data_usage.clone());
            *self.alerts.borrow_mut() = Some(shell.alerts.clone());
            *self.account.borrow_mut() = Some(shell.account.clone());
            shell.account.set_action_sender(self.cloud_actions.clone());
            shell.account.set_state(self.cloud_state.borrow().clone());
            *self.network.borrow_mut() = Some(shell.network.clone());
            shell.network.set_cloud_connected(matches!(
                &*self.cloud_state.borrow(),
                CloudState::Authenticated | CloudState::Ready { .. }
            ));
            *self.speed_test.borrow_mut() = Some(shell.speed_test.clone());
            shell
                .speed_test
                .set_samples(self.dashboard_samples.borrow().clone());
            *self.alignment.borrow_mut() = Some(shell.alignment.clone());
            *self.settings.borrow_mut() = Some(shell.settings.clone());
            // The first start after logging in is where an install finishes.
            self.refresh_shell_extension();
            // Seed the freshly built surfaces with the latest replies; the
            // recorder may have been running long before the window opened.
            shell
                .dashboard
                .set_status(self.latest_status.borrow().clone());
            shell
                .dashboard
                .set_obstruction(self.obstruction_map.borrow().clone());
            let stack = shell.stack.clone();
            shell
                .dashboard
                .connect_open_sky(move || stack.set_visible_child_name("satellite"));
            shell.sky.set_status(self.latest_status.borrow().clone());
            shell
                .sky
                .set_obstruction(self.obstruction_map.borrow().clone());
            let saved_location = self.preferences.borrow().get().observer_location;
            shell
                .sky
                .start_constellation(Some(self.paths.tle_cache_file()), saved_location);
            let state = Rc::downgrade(self);
            shell.sky.connect_location_changed(move |location| {
                let Some(state) = state.upgrade() else {
                    return;
                };
                let mut preferences = state.preferences.borrow_mut();
                let result = match location {
                    Some(location) => observer_location::save_location(&mut preferences, location),
                    None => observer_location::clear_saved_location(&mut preferences),
                };
                if let Err(error) = result {
                    tracing::warn!("could not save the observer location: {error}");
                }
            });
            shell
                .alignment
                .set_status(self.latest_status.borrow().clone());
            if let Some(snapshot) = self.latest_alerts.borrow().as_ref() {
                shell.alerts.set_snapshot(snapshot, &translator, now_ms());
            }
            if let Some(snapshot) = self.latest_network.borrow().as_deref() {
                if self.network_cloud_roster_displayed.get() {
                    shell.network.set_cloud_snapshot(snapshot.clone());
                } else {
                    shell.network.set_snapshot(snapshot.clone());
                }
            }
            let recorder_commands = self
                .recorder
                .borrow()
                .as_ref()
                .map(RecorderThread::command_sender);
            shell
                .data_usage
                .set_command_sender(recorder_commands.clone());
            shell.network.set_command_sender(recorder_commands.clone());
            shell
                .dashboard
                .set_command_sender(recorder_commands.clone());
            shell
                .network
                .set_cloud_action_sender(Some(self.network_cloud_actions.clone()));
            shell.settings.set_command_sender(recorder_commands);
            if let Some(sender) = self
                .recorder
                .borrow()
                .as_ref()
                .map(RecorderThread::command_sender)
            {
                let settings_demand = sender.clone();
                shell.stack.connect_visible_child_name_notify(move |stack| {
                    let wanted = stack.visible_child_name().as_deref() == Some("settings");
                    let _ = settings_demand.send(RecorderCommand::DishConfigDemand(wanted));
                });
            }
            shell
                .settings
                .set_action_sender(Some(self.settings_actions.clone()));
            shell.settings.set_account_connected(matches!(
                &*self.cloud_state.borrow(),
                CloudState::Authenticated | CloudState::Ready { .. }
            ));
            let latest_network = self.latest_network.borrow().clone();
            shell
                .settings
                .set_network_snapshot(latest_network.as_deref());
            shell.settings.set_app_state(self.settings_state());
            shell
                .network
                .set_self_client_id(self.preferences.borrow().get().self_client_id);
            shell
                .settings
                .set_status(self.latest_status.borrow().clone());
            shell
                .settings
                .set_router_reachable(self.latest_router_reachable.get());
            self.refresh_header_status(None);
            if let Some(snapshot) = self.latest_alerts.borrow().as_ref()
                && let Some(text) = ui::notification_badge_text(snapshot.active.len())
            {
                shell.notification_badge.set_label(&text);
                shell.notification_badge.set_visible(true);
            }

            let theme_state = self.clone();
            shell
                .theme_button
                .connect_clicked(move |_| theme_state.cycle_theme());

            let close_state = self.clone();
            shell.window.connect_close_request(move |window| {
                close_state.save_bounds(window);
                glib::Propagation::Proceed
            });

            let _ = self.window.borrow_mut().replace(shell.window);
        }
        if let Some(window) = self.window.borrow().as_ref() {
            window.present();
        }
        self.refresh_dashboard(now_ms());
    }

    fn cycle_theme(self: &Rc<Self>) {
        let theme = {
            let mut preferences = self.preferences.borrow_mut();
            let next = preferences.get().theme.next();
            if let Err(error) = preferences.set(|preferences| preferences.theme = next) {
                tracing::warn!("could not save theme: {error}");
            }
            next
        };
        self.apply_theme(theme);
        if let Some(button) = self.theme_button.borrow().as_ref() {
            let translator = self.translator.borrow();
            ui::set_theme_button(button, &translator, theme);
        }
    }

    fn apply_theme(&self, theme: Theme) {
        let scheme = match theme {
            Theme::Light => adw::ColorScheme::ForceLight,
            Theme::Dark => adw::ColorScheme::ForceDark,
            Theme::System => adw::ColorScheme::Default,
        };
        adw::StyleManager::default().set_color_scheme(scheme);
    }

    fn save_bounds(&self, window: &adw::ApplicationWindow) {
        if window.is_maximized() || window.is_fullscreen() {
            return;
        }
        let width = window.width();
        let height = window.height();
        if width <= 0 || height <= 0 {
            return;
        }
        // GTK4 has no window-position setter (Wayland forbids it), so the saved
        // x/y is kept but the compositor decides placement. The size is restored.
        let (x, y) = self
            .preferences
            .borrow()
            .get()
            .window_bounds
            .map_or((0, 0), |bounds| (bounds.x, bounds.y));
        let mut preferences = self.preferences.borrow_mut();
        if let Err(error) = preferences.set(|preferences| {
            preferences.window_bounds =
                Some(crate::preferences::WindowBounds::new(x, y, width, height));
        }) {
            tracing::warn!("could not save window bounds: {error}");
        }
    }

    fn tray_state(&self) -> TrayState {
        TrayState {
            notifications: notifications_requested(self.preferences.borrow().get().notifications),
            autostart: Autostart::new(self.paths.autostart_file()).is_installed(),
        }
    }

    fn refresh_tray(&self, translator: Option<Translator>) {
        let state = self.tray_state();
        let throughput = if self.preferences.borrow().get().menu_bar_throughput {
            self.tray_readout.borrow().clone()
        } else {
            String::new()
        };
        if let Some(tray) = self.tray.borrow().as_ref() {
            let _ = tray.update(|tray| tray.refresh(state, throughput, translator));
        }
    }

    fn set_notifications(&self, on: bool) {
        let mut preferences = self.preferences.borrow_mut();
        if let Err(error) = preferences.set(|preferences| preferences.notifications = Some(on)) {
            tracing::warn!("could not save notification preference: {error}");
        }
        drop(preferences);

        if on {
            let translator = self.translator.borrow();
            let title = translator.text("notifications.enabledTitle");
            let body = translator.text("notifications.enabledBody");
            drop(translator);
            self.post_notification("notifications-enabled", &title, &body);
        }
    }

    fn set_autostart(&self, on: bool) {
        let autostart = Autostart::new(self.paths.autostart_file());
        let result = if on {
            let app_name = self.translator.borrow().text("desktop.appName");
            crate::autostart::Launch::current()
                .and_then(|launch| autostart.install(&launch, &app_name))
        } else {
            autostart.remove().map(|_| ())
        };
        if let Err(error) = result {
            tracing::warn!("could not change start-at-login: {error}");
        }
    }

    fn post_notification(&self, key: &str, title: &str, body: &str) {
        if !self.notifier.borrow_mut().allow(key, now_ms()) {
            return;
        }
        let notification = gio::Notification::new(title);
        notification.set_body(Some(body));
        self.app.send_notification(Some(key), &notification);
    }

    fn quit(&self) {
        self.hold.borrow_mut().take();
        self.app.quit();
    }

    fn shutdown(&self) {
        if let Some(window) = self.window.borrow().as_ref() {
            self.save_bounds(window);
        }
        if let Some(mut recorder) = self.recorder.borrow_mut().take() {
            recorder.stop();
        }
        if let Some(tray) = self.tray.borrow_mut().take() {
            tray.shutdown().wait();
        }
        self.hold.borrow_mut().take();
    }
}

fn connected_displays() -> Vec<Display> {
    let Some(display) = gdk::Display::default() else {
        return Vec::new();
    };
    let monitors = display.monitors();
    let mut displays = Vec::new();
    for index in 0..monitors.n_items() {
        let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() else {
            continue;
        };
        let geometry = monitor.geometry();
        displays.push(Display::new(
            geometry.x(),
            geometry.y(),
            geometry.width(),
            geometry.height(),
        ));
    }
    displays
}

fn cloud_network_reads_allowed(
    network_visible: bool,
    lan_reachable: Option<bool>,
    account_connected: bool,
) -> bool {
    network_visible && lan_reachable == Some(false) && account_connected
}

fn cloud_roster_reads_allowed(
    network_visible: bool,
    lan_reachable: Option<bool>,
    account_connected: bool,
    requested: bool,
) -> bool {
    requested && cloud_network_reads_allowed(network_visible, lan_reachable, account_connected)
}

fn router_config_operation(update: &RouterConfigUpdate) -> &'static str {
    match update {
        RouterConfigUpdate::CustomDns { .. } => "custom_dns",
        RouterConfigUpdate::Bypass { .. } => "bypass",
        RouterConfigUpdate::FactoryReset => "factory_reset",
        RouterConfigUpdate::Subnet { .. } => "subnet",
        RouterConfigUpdate::WifiPassword { .. } => "wifi_password",
        RouterConfigUpdate::MeshName { .. } => "mesh_name",
        RouterConfigUpdate::ClientPause { .. } => "client_pause",
        RouterConfigUpdate::ClientRename { .. } => "client_rename",
    }
}

fn router_subnet_mismatch(requested: &str, config: &WifiConfig) -> Option<(String, String)> {
    let actual = config.subnet().unwrap_or("unknown");
    (actual != requested).then(|| (requested.to_owned(), actual.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{
        CloudRequest, WifiConfig, newest_connect, router_config_operation, router_subnet_mismatch,
    };
    use dishy_cloud::router_config::RouterConfigUpdate;

    // SPEC-CLOUD-011: queued sign-in snapshots collapse to the newest; other
    // requests keep their order.
    #[test]
    fn sign_in_verifies_only_the_newest_snapshot() {
        let (sender, receiver) = std::sync::mpsc::channel();
        sender.send(CloudRequest::Connect("second".into())).unwrap();
        sender.send(CloudRequest::Refresh).unwrap();
        sender.send(CloudRequest::Connect("third".into())).unwrap();
        let mut deferred = std::collections::VecDeque::new();
        let cookie = newest_connect("first".into(), &receiver, &mut deferred);
        assert_eq!(cookie, "third");
        assert!(matches!(deferred.pop_front(), Some(CloudRequest::Refresh)));
        assert!(deferred.is_empty());
    }

    // SPEC-CLOUDCFG-003: a successful RPC is not shown as applied when the
    // authenticated read-back still reports the old subnet.
    #[test]
    fn subnet_write_requires_readback_confirmation() {
        let config: WifiConfig = serde_json::from_value(serde_json::json!({
            "networks": [{"ipv4": "10.0.0.1/16"}]
        }))
        .unwrap();
        assert_eq!(
            router_subnet_mismatch("192.168.1.1/24", &config),
            Some(("192.168.1.1/24".into(), "10.0.0.1/16".into()))
        );
        assert_eq!(router_subnet_mismatch("10.0.0.1/16", &config), None);
        assert_eq!(
            router_subnet_mismatch("192.168.1.1/24", &WifiConfig::default()),
            Some(("192.168.1.1/24".into(), "unknown".into()))
        );
    }

    #[test]
    fn router_config_operation_labels_do_not_include_sensitive_values() {
        assert_eq!(
            router_config_operation(&RouterConfigUpdate::Subnet {
                subnet: "192.168.1.1/24".into(),
                password: "not-for-logs".into(),
            }),
            "subnet"
        );
        assert_eq!(
            router_config_operation(&RouterConfigUpdate::CustomDns {
                nameservers: vec!["1.1.1.1".into()],
            }),
            "custom_dns"
        );
    }

    use super::{cloud_network_reads_allowed, cloud_roster_reads_allowed};

    #[test]
    fn cloud_network_reads_require_visible_page_lan_failure_and_account() {
        assert!(cloud_network_reads_allowed(true, Some(false), true));
        assert!(!cloud_network_reads_allowed(false, Some(false), true));
        assert!(!cloud_network_reads_allowed(true, Some(true), true));
        assert!(!cloud_network_reads_allowed(true, None, true));
        assert!(!cloud_network_reads_allowed(true, Some(false), false));
        assert!(!cloud_roster_reads_allowed(true, Some(false), true, false));
        assert!(cloud_roster_reads_allowed(true, Some(false), true, true));
    }
}
