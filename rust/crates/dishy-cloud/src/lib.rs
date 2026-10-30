//! Account and usage reads for `api.starlink.com`.
//!
//! The cloud client is deliberately independent of the LAN recorder and GTK.
//! Hosts provide the session store and transport; the GTK layer receives only
//! decoded JSON data, never the session cookie.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use dishy_core::http_body::{MAX_RESPONSE_BYTES, read_capped};
use dishy_proto::Schema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

pub mod dish_config;
pub mod router_config;

pub const CLOUD_API_ORIGIN: &str = "https://api.starlink.com";
const AUTH_URL: &str = "https://api.starlink.com/auth-rp/auth/user";
const API: &str = "https://starlink.com/api";
const DEVICE_HANDLE_URL: &str = "https://starlink.com/api/SpaceX.API.Device.Device/Handle";
const REFRESH_TTL: Duration = Duration::from_secs(60);
const IDS_TTL: Duration = Duration::from_secs(5 * 60);
const CLOUD_CALL_TIMEOUT: Duration = Duration::from_secs(15);
const CONNECT_ATTEMPTS: usize = 4;
const CONNECT_RETRY_DELAY: Duration = Duration::from_millis(700);
const SERVICE_LINES_PATH: &str = "/webagg/v2/accounts/service-lines?limit=100&page=0&isConverting=false&serviceAddressId=&onlyActive=false&searchString=&onlyNoUts=false";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub set_cookie: Option<String>,
    pub body: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpFailure(pub String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrpcHttpResponse {
    pub status: u16,
    pub grpc_status: Option<u16>,
    pub grpc_message: Option<String>,
    pub body: Vec<u8>,
}

/// Host-owned credential persistence. Linux production wiring will use the
/// desktop secret service; tests and short-lived clients can use an in-memory
/// implementation.
pub trait SessionStore: Send + Sync {
    fn read_cookie(&self) -> Result<Option<String>, String>;
    fn write_cookie(&self, cookie: &str) -> Result<(), String>;
    fn clear_cookie(&self) -> Result<(), String>;
}

/// Narrow HTTP boundary so the account flow can be tested against scripted
/// replies and never needs a real Starlink account in CI.
#[allow(async_fn_in_trait)] // Generic transport, intentionally not used as a trait object.
pub trait CloudTransport: Send + Sync {
    async fn get(&self, url: &str, cookie: &str) -> Result<HttpResponse, HttpFailure>;
    async fn post_json(
        &self,
        url: &str,
        cookie: &str,
        body: &Value,
    ) -> Result<HttpResponse, HttpFailure>;

    async fn post_grpc_web(
        &self,
        url: &str,
        cookie: &str,
        framed_body: &[u8],
    ) -> Result<GrpcHttpResponse, HttpFailure>;
}

#[derive(Clone)]
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(CLOUD_CALL_TIMEOUT)
                .build()
                .expect("static cloud HTTP client configuration"),
        }
    }
}

// SPEC-BODY-001: cloud replies go through the same capped reader as the LAN.
async fn read_body(response: reqwest::Response) -> Result<Vec<u8>, HttpFailure> {
    read_capped(response, MAX_RESPONSE_BYTES)
        .await
        .map_err(|error| HttpFailure(error.to_string()))
}

impl CloudTransport for ReqwestTransport {
    async fn get(&self, url: &str, cookie: &str) -> Result<HttpResponse, HttpFailure> {
        let response = self
            .client
            .get(url)
            .header(reqwest::header::COOKIE, cookie)
            .header(reqwest::header::ACCEPT, "application/json")
            .send()
            .await
            .map_err(|error| HttpFailure(error.to_string()))?;
        let status = response.status().as_u16();
        let set_cookie = response
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = String::from_utf8_lossy(&read_body(response).await?).into_owned();
        Ok(HttpResponse {
            status,
            set_cookie,
            body,
        })
    }

    async fn post_json(
        &self,
        url: &str,
        cookie: &str,
        body: &Value,
    ) -> Result<HttpResponse, HttpFailure> {
        let response = self
            .client
            .post(url)
            .header(reqwest::header::COOKIE, cookie)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(serde_json::to_vec(body).map_err(|error| HttpFailure(error.to_string()))?)
            .send()
            .await
            .map_err(|error| HttpFailure(error.to_string()))?;
        let status = response.status().as_u16();
        let set_cookie = response
            .headers()
            .get(reqwest::header::SET_COOKIE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = String::from_utf8_lossy(&read_body(response).await?).into_owned();
        Ok(HttpResponse {
            status,
            set_cookie,
            body,
        })
    }

    async fn post_grpc_web(
        &self,
        url: &str,
        cookie: &str,
        framed_body: &[u8],
    ) -> Result<GrpcHttpResponse, HttpFailure> {
        let response = self
            .client
            .post(url)
            .header(reqwest::header::COOKIE, cookie)
            .header(reqwest::header::ACCEPT, "application/grpc-web+proto")
            .header(reqwest::header::CONTENT_TYPE, "application/grpc-web+proto")
            .header("x-grpc-web", "1")
            .body(framed_body.to_vec())
            .send()
            .await
            .map_err(|error| HttpFailure(error.to_string()))?;
        let status = response.status().as_u16();
        let grpc_status = response
            .headers()
            .get("grpc-status")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.parse().ok());
        let grpc_message = response
            .headers()
            .get("grpc-message")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = read_body(response).await?;
        Ok(GrpcHttpResponse {
            status,
            grpc_status,
            grpc_message,
            body,
        })
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CloudError {
    #[error("not_connected")]
    NotConnected,
    #[error("session_expired")]
    SessionExpired,
    #[error("no service line on this account")]
    NoServiceLine,
    #[error("cloud request failed: {0}")]
    Transport(String),
    #[error("cloud API returned HTTP {0}")]
    Http(u16),
    #[error("cloud API returned invalid JSON: {0}")]
    InvalidJson(String),
    #[error("session store failed: {0}")]
    SessionStore(String),
    #[error("Starlink has not reported a router controller yet")]
    ControllerUnknown,
    #[error("Starlink has not reported a dish terminal yet")]
    DishUnknown,
    #[error("router controller is unreachable")]
    DeviceUnreachable,
    #[error("grpc device call failed ({code}): {message}")]
    GrpcStatus { code: u16, message: String },
    #[error("grpc-web response is malformed: {0}")]
    GrpcProtocol(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountData {
    pub identity: Option<Value>,
    pub service_line: Option<Value>,
    pub device_telemetry: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ServiceLineIds {
    account: String,
    service_line: String,
}

struct Cached<T> {
    value: T,
    at: Instant,
}

/// Authenticated cloud API client. A generic host-provided store makes the
/// credential boundary explicit and keeps persistent-secret details out of the
/// account protocol.
pub struct CloudClient<T: CloudTransport, S: SessionStore> {
    transport: Arc<T>,
    session: Arc<S>,
    token: Mutex<Option<Cached<String>>>,
    ids: Mutex<Option<Cached<ServiceLineIds>>>,
    refresh_lock: tokio::sync::Mutex<()>,
    schema: Schema,
}

impl<T: CloudTransport, S: SessionStore> CloudClient<T, S> {
    pub fn new(transport: Arc<T>, session: Arc<S>) -> Self {
        Self {
            transport,
            session,
            token: Mutex::new(None),
            ids: Mutex::new(None),
            refresh_lock: tokio::sync::Mutex::new(()),
            schema: Schema::bundled().expect("bundled device schema"),
        }
    }

    /// Validate and persist a browser-obtained session, then verify that it can
    /// resolve at least one active service line.
    pub async fn connect(&self, cookie: &str) -> Result<(), CloudError> {
        let cookie = cookie.trim();
        if !cookie.contains("Starlink.Com.Sso=") {
            return Err(CloudError::NotConnected);
        }
        self.session
            .write_cookie(cookie)
            .map_err(CloudError::SessionStore)?;
        let mut result = Err(CloudError::NotConnected);
        for attempt in 0..CONNECT_ATTEMPTS {
            self.invalidate();
            result = self
                .with_cookie(|cookie| async move { self.resolve_ids(&cookie).await.map(|_| ()) })
                .await;
            if !matches!(
                result,
                Err(CloudError::SessionExpired | CloudError::NotConnected)
            ) {
                break;
            }
            if attempt + 1 < CONNECT_ATTEMPTS {
                tokio::time::sleep(CONNECT_RETRY_DELAY).await;
            }
        }
        if matches!(
            result,
            Err(CloudError::SessionExpired | CloudError::NotConnected)
        ) {
            self.invalidate();
            self.session
                .clear_cookie()
                .map_err(CloudError::SessionStore)?;
        }
        result
    }

    pub fn disconnect(&self) -> Result<(), CloudError> {
        self.invalidate();
        self.session
            .clear_cookie()
            .map_err(CloudError::SessionStore)
    }

    pub fn is_connected(&self) -> Result<bool, CloudError> {
        self.session
            .read_cookie()
            .map(|cookie| cookie.is_some())
            .map_err(CloudError::SessionStore)
    }

    /// Read the current account and primary service line. Cloud telemetry is
    /// optional here so plan details remain available during telemetry outages.
    pub async fn account(&self) -> Result<AccountData, CloudError> {
        self.with_cookie(|cookie| async move {
            let ids = self.resolve_ids(&cookie).await?;
            let identity = match self.identity(&cookie).await {
                Ok(identity) => Some(identity),
                Err(CloudError::SessionExpired) => return Err(CloudError::SessionExpired),
                Err(_) => None,
            };
            let service_line = self
                .api_get(
                    &format!("{API}/webagg/v2/accounts/service-line/{}", ids.service_line),
                    &cookie,
                )
                .await?;
            let device_telemetry = self
                .api_post(
                    &format!("{API}/device-data/cache/v1/telemetry"),
                    &cookie,
                    serde_json::json!({"accountNumber": ids.account}),
                )
                .await;
            let device_telemetry = match device_telemetry {
                Ok(value) => device_telemetry_from(&value),
                Err(CloudError::SessionExpired) => return Err(CloudError::SessionExpired),
                Err(_) => serde_json::json!({}),
            };
            Ok(AccountData {
                identity,
                service_line: Some(service_line),
                device_telemetry,
            })
        })
        .await
    }

    /// Return the Starlink annotated billing-cycle usage payload unchanged for
    /// the shared data-usage view model to interpret.
    pub async fn usage(&self) -> Result<Value, CloudError> {
        self.with_cookie(|cookie| async move {
            let ids = self.resolve_ids(&cookie).await?;
            self.api_get(
                &format!(
                    "{API}/telemetryagg/v1/data-usage/account/{}/service-line/{}/annotated",
                    ids.account, ids.service_line
                ),
                &cookie,
            )
            .await
        })
        .await
    }

    /// Send one schema-encoded router call through Starlink's authenticated
    /// gateway. The controller is deliberately resolved from fresh telemetry
    /// for every operation: a cached mesh node can be the wrong write target
    /// during a bypass or topology change.
    pub async fn router_call(&self, request: &Value) -> Result<Value, CloudError> {
        if !request.is_object() || request.as_object().is_none_or(|object| object.is_empty()) {
            return Err(CloudError::GrpcProtocol(
                "request must be a non-empty object".into(),
            ));
        }
        self.with_cookie(|cookie| async move {
            let target_id = self.resolve_controller_id(&cookie).await?;
            self.device_call_at(&cookie, &target_id, request).await
        })
        .await
    }

    /// Read connected Wi-Fi clients through the authenticated device gateway.
    /// An omitted proto3 repeated field is the valid empty roster.
    pub async fn router_wifi_clients(&self) -> Result<Value, CloudError> {
        let response = self
            .router_call(&serde_json::json!({"wifiGetClients": {}}))
            .await?;
        router_wifi_clients_from(&response)
    }

    /// Read Wi-Fi configuration through the authenticated device gateway.
    /// An omitted proto3 message is represented as an empty object.
    pub async fn router_wifi_config(&self) -> Result<Value, CloudError> {
        let response = self
            .router_call(&serde_json::json!({"wifiGetConfig": {}}))
            .await?;
        router_wifi_config_from(&response)
    }

    /// Apply one typed router setting through the authenticated gateway. The
    /// controller id in the built request is only a validated placeholder:
    /// `router_call` replaces it with the controller freshly resolved from
    /// account telemetry immediately before encoding and dispatch.
    pub async fn update_router_config(
        &self,
        update: router_config::RouterConfigUpdate,
    ) -> Result<Value, CloudError> {
        router_config::validate_router_config_update(&update)?;
        self.with_cookie(|cookie| {
            let update = update.clone();
            async move {
                let target_id = self.resolve_controller_id(&cookie).await?;
                let needs_context = matches!(
                    &update,
                    router_config::RouterConfigUpdate::Subnet { .. }
                        | router_config::RouterConfigUpdate::WifiPassword { .. }
                        | router_config::RouterConfigUpdate::MeshName { .. }
                        | router_config::RouterConfigUpdate::ClientPause { .. }
                        | router_config::RouterConfigUpdate::ClientRename { .. }
                );
                let wifi_config = if needs_context {
                    self.device_call_at(
                        &cookie,
                        &target_id,
                        &serde_json::json!({"wifiGetConfig": {}}),
                    )
                    .await?
                    .get("wifiGetConfig")
                    .and_then(|reply| reply.get("wifiConfig"))
                    .cloned()
                    .unwrap_or(Value::Null)
                } else {
                    Value::Null
                };
                let request = router_config::build_router_config_request_with_context(
                    &target_id,
                    update,
                    &wifi_config,
                )?;
                self.device_call_at(&cookie, &target_id, &request).await
            }
        })
        .await
    }

    /// Apply a typed dish setting. The dish identity is selected from fresh
    /// authenticated account telemetry; the GTK side never supplies a target
    /// ID or protobuf bytes.
    pub async fn update_dish_config(
        &self,
        update: dish_config::DishConfigUpdate,
    ) -> Result<Value, CloudError> {
        dish_config::validate(&update)?;
        self.with_cookie(|cookie| {
            let update = update.clone();
            async move {
                let target_id = self.resolve_dish_id(&cookie).await?;
                let request = dish_config::build_request(&target_id, update)?;
                self.device_call_at(&cookie, &target_id, &request).await
            }
        })
        .await
    }

    async fn device_call_at(
        &self,
        cookie: &str,
        target_id: &str,
        request: &Value,
    ) -> Result<Value, CloudError> {
        if !request.is_object() || request.as_object().is_none_or(|object| object.is_empty()) {
            return Err(CloudError::GrpcProtocol(
                "request must be a non-empty object".into(),
            ));
        }
        let mut request = request.clone();
        request["targetId"] = Value::String(target_id.to_owned());
        let bytes = self
            .schema
            .encode_request_json(&request)
            .map_err(|error| CloudError::GrpcProtocol(error.to_string()))?;
        let framed = encode_grpc_message(&bytes);
        let response = self
            .transport
            .post_grpc_web(DEVICE_HANDLE_URL, cookie, &framed)
            .await
            .map_err(|error| CloudError::Transport(error.0))?;
        if let Some(code) = response.grpc_status
            && code != 0
        {
            return Err(device_grpc_error(
                code,
                response.grpc_message.unwrap_or_default(),
            ));
        }
        if response.status == 401 || response.status == 403 {
            return Err(CloudError::SessionExpired);
        }
        if !(200..300).contains(&response.status) {
            return Err(CloudError::Http(response.status));
        }
        let payload = decode_grpc_response(&response.body)?;
        self.schema
            .decode_response_json(&payload)
            .map_err(|error| CloudError::GrpcProtocol(error.to_string()))
    }

    async fn resolve_controller_id(&self, cookie: &str) -> Result<String, CloudError> {
        let ids = self.resolve_ids(cookie).await?;
        let telemetry = self
            .api_post(
                &format!("{API}/device-data/cache/v1/telemetry"),
                cookie,
                serde_json::json!({"accountNumber": ids.account}),
            )
            .await?;
        let devices = device_telemetry_from(&telemetry);
        devices
            .as_object()
            .and_then(|devices| {
                devices.iter().find_map(|(id, device)| {
                    (device.get("kind").and_then(Value::as_str) == Some("router")
                        && device
                            .get("hops")
                            .and_then(Value::as_f64)
                            .is_some_and(|hops| hops == 0.0))
                    .then(|| id.clone())
                })
            })
            .ok_or(CloudError::ControllerUnknown)
    }

    async fn resolve_dish_id(&self, cookie: &str) -> Result<String, CloudError> {
        let ids = self.resolve_ids(cookie).await?;
        let telemetry = self
            .api_post(
                &format!("{API}/device-data/cache/v1/telemetry"),
                cookie,
                serde_json::json!({"accountNumber": ids.account}),
            )
            .await?;
        let devices = device_telemetry_from(&telemetry);
        devices
            .as_object()
            .and_then(|devices| {
                devices.iter().find_map(|(id, device)| {
                    (device.get("kind").and_then(Value::as_str) == Some("dish")).then(|| id.clone())
                })
            })
            .ok_or(CloudError::DishUnknown)
    }

    fn invalidate(&self) {
        *self.token.lock().expect("cloud token lock") = None;
        *self.ids.lock().expect("cloud ids lock") = None;
    }

    async fn with_cookie<R, F, Fut>(&self, run: F) -> Result<R, CloudError>
    where
        F: Fn(String) -> Fut,
        Fut: std::future::Future<Output = Result<R, CloudError>>,
    {
        let first = match self.fresh_cookie(false).await {
            Ok(cookie) => run(cookie).await,
            Err(error) => Err(error),
        };
        match first {
            Ok(value) => Ok(value),
            Err(CloudError::SessionExpired) => run(self.fresh_cookie(true).await?).await,
            Err(error) => Err(error),
        }
    }

    async fn fresh_cookie(&self, force: bool) -> Result<String, CloudError> {
        let _guard = self.refresh_lock.lock().await;
        if !force
            && let Some(cached) = self.token.lock().expect("cloud token lock").as_ref()
            && cached.at.elapsed() < REFRESH_TTL
        {
            return Ok(cached.value.clone());
        }
        let base = self
            .session
            .read_cookie()
            .map_err(CloudError::SessionStore)?
            .filter(|cookie| cookie.contains("Starlink.Com.Sso="))
            .ok_or(CloudError::NotConnected)?;
        let response = self
            .transport
            .get(AUTH_URL, &base)
            .await
            .map_err(|error| CloudError::Transport(error.0))?;
        if response.status == 401 || response.status == 403 {
            return Err(CloudError::SessionExpired);
        }
        if !(200..300).contains(&response.status) {
            return Err(CloudError::Http(response.status));
        }
        let refreshed = response
            .set_cookie
            .as_deref()
            .and_then(access_cookie)
            .map(|access| merge_cookie(&base, &access))
            .unwrap_or(base);
        *self.token.lock().expect("cloud token lock") = Some(Cached {
            value: refreshed.clone(),
            at: Instant::now(),
        });
        Ok(refreshed)
    }

    async fn resolve_ids(&self, cookie: &str) -> Result<ServiceLineIds, CloudError> {
        if let Some(cached) = self.ids.lock().expect("cloud ids lock").as_ref()
            && cached.at.elapsed() < IDS_TTL
        {
            return Ok(cached.value.clone());
        }
        let list = self
            .api_get(&format!("{API}{SERVICE_LINES_PATH}"), cookie)
            .await?;
        let first = list
            .pointer("/content/results/0")
            .ok_or(CloudError::NoServiceLine)?;
        let account = first
            .get("accountReferenceId")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(CloudError::NoServiceLine)?;
        let service_line = first
            .get("serviceLineNumber")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .ok_or(CloudError::NoServiceLine)?;
        let ids = ServiceLineIds {
            account: account.to_owned(),
            service_line: service_line.to_owned(),
        };
        *self.ids.lock().expect("cloud ids lock") = Some(Cached {
            value: ids.clone(),
            at: Instant::now(),
        });
        Ok(ids)
    }

    async fn identity(&self, cookie: &str) -> Result<Value, CloudError> {
        let response = self
            .transport
            .get(AUTH_URL, cookie)
            .await
            .map_err(|error| CloudError::Transport(error.0))?;
        self.decode_response(response)
    }

    async fn api_get(&self, url: &str, cookie: &str) -> Result<Value, CloudError> {
        let response = self
            .transport
            .get(url, cookie)
            .await
            .map_err(|error| CloudError::Transport(error.0))?;
        self.decode_response(response)
    }

    async fn api_post(&self, url: &str, cookie: &str, body: Value) -> Result<Value, CloudError> {
        let response = self
            .transport
            .post_json(url, cookie, &body)
            .await
            .map_err(|error| CloudError::Transport(error.0))?;
        self.decode_response(response)
    }

    fn decode_response(&self, response: HttpResponse) -> Result<Value, CloudError> {
        if response.status == 401 || response.status == 403 {
            return Err(CloudError::SessionExpired);
        }
        if !(200..300).contains(&response.status) {
            return Err(CloudError::Http(response.status));
        }
        serde_json::from_str(&response.body)
            .map_err(|error| CloudError::InvalidJson(error.to_string()))
    }
}

fn router_wifi_clients_from(response: &Value) -> Result<Value, CloudError> {
    let Some(reply) = response.get("wifiGetClients") else {
        return Ok(serde_json::json!([]));
    };
    let Some(reply) = reply.as_object() else {
        return Err(CloudError::GrpcProtocol(
            "wifiGetClients reply must be an object".into(),
        ));
    };
    match reply.get("clients") {
        None => Ok(serde_json::json!([])),
        Some(Value::Array(clients)) => Ok(Value::Array(clients.clone())),
        Some(_) => Err(CloudError::GrpcProtocol(
            "wifiGetClients.clients must be an array".into(),
        )),
    }
}

fn router_wifi_config_from(response: &Value) -> Result<Value, CloudError> {
    let Some(reply) = response.get("wifiGetConfig") else {
        return Ok(serde_json::json!({}));
    };
    let Some(reply) = reply.as_object() else {
        return Err(CloudError::GrpcProtocol(
            "wifiGetConfig reply must be an object".into(),
        ));
    };
    match reply.get("wifiConfig") {
        None => Ok(serde_json::json!({})),
        Some(Value::Object(config)) => Ok(Value::Object(config.clone())),
        Some(_) => Err(CloudError::GrpcProtocol(
            "wifiGetConfig.wifiConfig must be an object".into(),
        )),
    }
}

/// Convert Starlink's columnar device-cache payload to the renderer-facing map
/// keyed by the stable full DeviceId. This mirrors the Electron host contract.
pub fn device_telemetry_from(telemetry: &Value) -> Value {
    let Some(data) = telemetry.get("data") else {
        return serde_json::json!({});
    };
    let Some(columns) = data
        .get("columnNamesByDeviceType")
        .and_then(Value::as_object)
    else {
        return serde_json::json!({});
    };
    let Some(rows) = data.get("values").and_then(Value::as_array) else {
        return serde_json::json!({});
    };
    let mut output = serde_json::Map::new();
    for row in rows {
        let Some(values) = row.as_array() else {
            continue;
        };
        let Some(kind) = values.first().and_then(Value::as_str) else {
            continue;
        };
        let Some(legend) = columns.get(kind).and_then(Value::as_array) else {
            continue;
        };
        let field = |name: &str| -> Option<&Value> {
            let index = legend
                .iter()
                .position(|column| column.as_str() == Some(name))?;
            values.get(index)
        };
        let Some(id) = field("DeviceId").and_then(value_as_text) else {
            continue;
        };
        let timestamp_ms = field("UtcTimestampNs")
            .and_then(value_as_number)
            .unwrap_or(0.0)
            / 1_000_000.0;
        let mut item = serde_json::Map::new();
        if kind == "u" {
            item.insert("kind".into(), Value::String("dish".into()));
            item.insert("timestampMs".into(), serde_json::json!(timestamp_ms));
            for (name, source) in [
                ("softwareVersion", "RunningSoftwareVersion"),
                ("uptimeS", "Uptime"),
                ("obstructionPct", "ObstructionPercentTime"),
                ("signalQuality", "SignalQuality"),
            ] {
                if let Some(value) = field(source) {
                    let converted = if name == "softwareVersion" {
                        value_as_text(value).map(Value::String)
                    } else {
                        value_as_number(value).map(|number| serde_json::json!(number))
                    };
                    if let Some(converted) = converted {
                        item.insert(name.into(), converted);
                    }
                }
            }
        } else if kind == "r" {
            item.insert("kind".into(), Value::String("router".into()));
            item.insert("timestampMs".into(), serde_json::json!(timestamp_ms));
            for (name, source) in [
                ("hardwareVersion", "WifiHardwareVersion"),
                ("softwareVersion", "WifiSoftwareVersion"),
                ("uptimeS", "WifiUptimeS"),
                ("clients", "Clients"),
                ("hops", "WifiHopsFromController"),
            ] {
                if let Some(value) = field(source) {
                    let converted = if name.ends_with("Version") {
                        value_as_text(value).map(Value::String)
                    } else {
                        value_as_number(value).map(|number| serde_json::json!(number))
                    };
                    if let Some(converted) = converted {
                        item.insert(name.into(), converted);
                    }
                }
            }
            for (name, source) in [
                ("isRepeater", "WifiIsRepeater"),
                ("isBypassed", "WifiIsBypassed"),
            ] {
                if let Some(value) = field(source).and_then(Value::as_bool) {
                    item.insert(name.into(), Value::Bool(value));
                }
            }
        } else {
            continue;
        }
        output.insert(id, Value::Object(item));
    }
    Value::Object(output)
}

fn value_as_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn value_as_number(value: &Value) -> Option<f64> {
    let number = value
        .as_f64()
        .or_else(|| value.as_str()?.parse::<f64>().ok())?;
    number.is_finite().then_some(number)
}

fn encode_grpc_message(payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(payload.len() + 5);
    frame.push(0);
    frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn decode_grpc_response(body: &[u8]) -> Result<Vec<u8>, CloudError> {
    let mut offset = 0usize;
    let mut message = None;
    while offset < body.len() {
        let header = body
            .get(offset..offset + 5)
            .ok_or_else(|| CloudError::GrpcProtocol("truncated frame header".into()))?;
        let flag = header[0];
        let length = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        let start = offset + 5;
        let end = start
            .checked_add(length)
            .ok_or_else(|| CloudError::GrpcProtocol("frame length overflow".into()))?;
        let payload = body
            .get(start..end)
            .ok_or_else(|| CloudError::GrpcProtocol("truncated frame payload".into()))?;
        if flag == 0 {
            message = Some(payload.to_vec());
        } else if flag & 0x80 != 0 {
            let trailers = std::str::from_utf8(payload)
                .map_err(|_| CloudError::GrpcProtocol("trailers are not UTF-8".into()))?;
            let (status, text) = parse_grpc_trailers(trailers);
            if status != 0 {
                return Err(device_grpc_error(status, text));
            }
        } else {
            return Err(CloudError::GrpcProtocol(format!(
                "unsupported frame flag {flag}"
            )));
        }
        offset = end;
    }
    message.ok_or_else(|| CloudError::GrpcProtocol("response contained no message".into()))
}

fn parse_grpc_trailers(trailers: &str) -> (u16, String) {
    let mut status = 0;
    let mut message = String::new();
    for line in trailers.split("\r\n") {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "grpc-status" => status = value.trim().parse().unwrap_or(2),
            "grpc-message" => message = percent_decode(value.trim()),
            _ => {}
        }
    }
    (status, message)
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) = (hex(bytes[index + 1]), hex(bytes[index + 2]))
        {
            output.push((high << 4) | low);
            index += 3;
        } else {
            output.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8_lossy(&output).into_owned()
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn device_grpc_error(code: u16, message: String) -> CloudError {
    match code {
        5 => CloudError::DeviceUnreachable,
        16 => CloudError::SessionExpired,
        _ => CloudError::GrpcStatus { code, message },
    }
}

fn access_cookie(set_cookie: &str) -> Option<String> {
    let value = set_cookie
        .split(';')
        .next()?
        .trim()
        .strip_prefix("Starlink.Com.Access.V1=")?;
    (!value.is_empty()).then(|| format!("Starlink.Com.Access.V1={value}"))
}

fn merge_cookie(base: &str, access: &str) -> String {
    let retained = base
        .split(';')
        .map(str::trim)
        .filter(|part| !part.starts_with("Starlink.Com.Access.V1="))
        .collect::<Vec<_>>();
    std::iter::once(access)
        .chain(retained)
        .collect::<Vec<_>>()
        .join(";")
}

/// In-memory session storage for tests and for processes that intentionally do
/// not persist account credentials.
#[derive(Default)]
pub struct MemorySessionStore(Mutex<Option<String>>);

impl SessionStore for MemorySessionStore {
    fn read_cookie(&self) -> Result<Option<String>, String> {
        Ok(self.0.lock().map_err(|e| e.to_string())?.clone())
    }

    fn write_cookie(&self, cookie: &str) -> Result<(), String> {
        *self.0.lock().map_err(|e| e.to_string())? = Some(cookie.to_owned());
        Ok(())
    }

    fn clear_cookie(&self) -> Result<(), String> {
        *self.0.lock().map_err(|e| e.to_string())? = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakeTransport {
        replies: Mutex<VecDeque<HttpResponse>>,
        requests: Mutex<Vec<(String, String, Option<Value>)>>,
        grpc_replies: Mutex<VecDeque<GrpcHttpResponse>>,
        grpc_requests: Mutex<Vec<(String, String, Vec<u8>)>>,
    }

    impl FakeTransport {
        fn scripted(replies: Vec<HttpResponse>) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
                requests: Mutex::new(Vec::new()),
                grpc_replies: Mutex::new(VecDeque::new()),
                grpc_requests: Mutex::new(Vec::new()),
            }
        }

        fn scripted_with_grpc(
            replies: Vec<HttpResponse>,
            grpc_replies: Vec<GrpcHttpResponse>,
        ) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
                grpc_replies: Mutex::new(grpc_replies.into()),
                ..Self::default()
            }
        }

        fn reply(&self) -> Result<HttpResponse, HttpFailure> {
            self.replies
                .lock()
                .map_err(|error| HttpFailure(error.to_string()))?
                .pop_front()
                .ok_or_else(|| HttpFailure("unexpected request".into()))
        }
    }

    impl CloudTransport for FakeTransport {
        async fn get(&self, url: &str, cookie: &str) -> Result<HttpResponse, HttpFailure> {
            self.requests
                .lock()
                .map_err(|error| HttpFailure(error.to_string()))?
                .push((url.to_owned(), cookie.to_owned(), None));
            if url == AUTH_URL {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            self.reply()
        }

        async fn post_json(
            &self,
            url: &str,
            cookie: &str,
            body: &Value,
        ) -> Result<HttpResponse, HttpFailure> {
            self.requests
                .lock()
                .map_err(|error| HttpFailure(error.to_string()))?
                .push((url.to_owned(), cookie.to_owned(), Some(body.clone())));
            self.reply()
        }

        async fn post_grpc_web(
            &self,
            url: &str,
            cookie: &str,
            framed_body: &[u8],
        ) -> Result<GrpcHttpResponse, HttpFailure> {
            self.grpc_requests
                .lock()
                .map_err(|error| HttpFailure(error.to_string()))?
                .push((url.to_owned(), cookie.to_owned(), framed_body.to_vec()));
            self.grpc_replies
                .lock()
                .map_err(|error| HttpFailure(error.to_string()))?
                .pop_front()
                .ok_or_else(|| HttpFailure("unexpected grpc request".into()))
        }
    }

    fn response(status: u16, body: Value) -> HttpResponse {
        HttpResponse {
            status,
            set_cookie: None,
            body: body.to_string(),
        }
    }

    fn grpc_response(status: u16, body: Vec<u8>) -> GrpcHttpResponse {
        GrpcHttpResponse {
            status,
            grpc_status: None,
            grpc_message: None,
            body,
        }
    }

    fn refresh_response(access: &str) -> HttpResponse {
        HttpResponse {
            status: 200,
            set_cookie: Some(format!("Starlink.Com.Access.V1={access}; Path=/; HttpOnly")),
            body: "{}".into(),
        }
    }

    fn service_lines() -> HttpResponse {
        response(
            200,
            serde_json::json!({"content":{"results":[{
                "accountReferenceId":"ACC-1",
                "serviceLineNumber":"SL-1"
            }]}}),
        )
    }

    #[test]
    fn access_cookie_replaces_only_the_short_lived_token() {
        assert_eq!(
            merge_cookie(
                "Starlink.Com.Access.V1=old; Starlink.Com.Sso=durable",
                "Starlink.Com.Access.V1=new"
            ),
            "Starlink.Com.Access.V1=new;Starlink.Com.Sso=durable"
        );
        assert_eq!(
            access_cookie("Starlink.Com.Access.V1=abc; Path=/; HttpOnly").as_deref(),
            Some("Starlink.Com.Access.V1=abc")
        );
    }

    // SPEC-CLOUDGW-001: verify the production HTTP adapter sends the grpc-web
    // headers and host session, not only that the cloud client calls its trait.
    #[tokio::test]
    async fn reqwest_gateway_sends_grpc_web_headers_and_cookie() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0; 1024];
            let (header_end, content_length) = loop {
                if let Some(index) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                    let headers = std::str::from_utf8(&request[..index]).unwrap();
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    break (index, content_length);
                }
                let count = stream.read(&mut buffer).unwrap();
                assert_ne!(count, 0, "client closed before sending HTTP headers");
                request.extend_from_slice(&buffer[..count]);
            };
            let request_length = header_end + 4 + content_length;
            while request.len() < request_length {
                let count = stream.read(&mut buffer).unwrap();
                assert_ne!(count, 0, "client closed before sending grpc-web body");
                request.extend_from_slice(&buffer[..count]);
            }
            let body = [0, 0, 0, 0, 0];
            write!(
                stream,
                "HTTP/1.1 200 OK\r\ncontent-length: {}\r\ngrpc-status: 0\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .unwrap();
            stream.write_all(&body).unwrap();
            request
        });

        let response = ReqwestTransport::default()
            .post_grpc_web(
                &format!("http://{address}/device/handle"),
                "Starlink.Com.Sso=fake-session",
                &[0, 0, 0, 0, 0],
            )
            .await
            .unwrap();
        let request = server.join().unwrap();
        let headers_end = request
            .windows(4)
            .position(|bytes| bytes == b"\r\n\r\n")
            .unwrap();
        let headers = std::str::from_utf8(&request[..headers_end])
            .unwrap()
            .to_ascii_lowercase();
        assert!(headers.starts_with("post /device/handle http/1.1"));
        assert!(headers.contains("cookie: starlink.com.sso=fake-session"));
        assert!(headers.contains("accept: application/grpc-web+proto"));
        assert!(headers.contains("content-type: application/grpc-web+proto"));
        assert!(headers.contains("x-grpc-web: 1"));
        assert_eq!(&request[headers_end + 4..], &[0, 0, 0, 0, 0]);
        assert_eq!(response.status, 200);
        assert_eq!(response.grpc_status, Some(0));
    }

    // SPEC-CLOUDGW-001, SPEC-CLOUDGW-004: router calls are encoded with the
    // bundled schema and sent only through the fixed authenticated endpoint.
    #[tokio::test]
    async fn router_gateway_encodes_and_frames_against_a_fresh_controller() {
        let transport = Arc::new(FakeTransport::scripted_with_grpc(
            vec![
                refresh_response("access"),
                service_lines(),
                response(
                    200,
                    serde_json::json!({"data": {
                        "columnNamesByDeviceType": {
                            "r": ["kind", "DeviceId", "UtcTimestampNs", "WifiHopsFromController"]
                        },
                        "values": [
                            ["r", "Router-mesh", 2_000_000_000, 1],
                            ["r", "Router-controller", 2_000_000_000, 0]
                        ]
                    }}),
                ),
            ],
            vec![grpc_response(200, encode_grpc_message(&[]))],
        ));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);

        assert_eq!(
            client
                .router_call(&serde_json::json!({"wifiGetConfig": {}}))
                .await
                .unwrap(),
            serde_json::json!({})
        );
        let requests = transport.grpc_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, DEVICE_HANDLE_URL);
        assert!(requests[0].1.contains("Starlink.Com.Sso=session"));
        assert!(requests[0].1.contains("Access.V1=access"));
        assert_eq!(requests[0].2[0], 0);
        let payload_len = u32::from_be_bytes(requests[0].2[1..5].try_into().unwrap()) as usize;
        assert_eq!(payload_len + 5, requests[0].2.len());
        assert!(!requests[0].2[5..].is_empty());
    }

    // SPEC-NETCLOUD-003: roster and config reads use their exact schema fields
    // and the fresh controller selected for each authenticated gateway call.
    #[tokio::test]
    async fn router_network_reads_use_the_authenticated_gateway() {
        let controller_telemetry = || {
            response(
                200,
                serde_json::json!({"data": {
                    "columnNamesByDeviceType": {
                        "r": ["kind", "DeviceId", "UtcTimestampNs", "WifiHopsFromController"]
                    },
                    "values": [["r", "Router-controller", 2_000_000_000, 0]]
                }}),
            )
        };
        let transport = Arc::new(FakeTransport::scripted_with_grpc(
            vec![
                refresh_response("access"),
                service_lines(),
                controller_telemetry(),
                controller_telemetry(),
            ],
            vec![
                grpc_response(200, encode_grpc_message(&[])),
                grpc_response(200, encode_grpc_message(&[])),
            ],
        ));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);

        assert_eq!(
            client.router_wifi_clients().await.unwrap(),
            serde_json::json!([])
        );
        assert_eq!(
            client.router_wifi_config().await.unwrap(),
            serde_json::json!({})
        );

        let schema = Schema::bundled().unwrap();
        let requests = transport.grpc_requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        for ((url, cookie, framed), request) in requests.iter().zip([
            serde_json::json!({"wifiGetClients": {}, "targetId": "Router-controller"}),
            serde_json::json!({"wifiGetConfig": {}, "targetId": "Router-controller"}),
        ]) {
            assert_eq!(url, DEVICE_HANDLE_URL);
            assert!(cookie.contains("Starlink.Com.Sso=session"));
            assert!(cookie.contains("Access.V1=access"));
            let length = u32::from_be_bytes(framed[1..5].try_into().unwrap()) as usize;
            assert_eq!(length + 5, framed.len());
            assert_eq!(
                &framed[5..],
                schema.encode_request_json(&request).unwrap().as_slice()
            );
        }
    }

    #[tokio::test]
    async fn router_network_reads_preserve_missing_session_errors() {
        let transport = Arc::new(FakeTransport::default());
        let client = CloudClient::new(transport.clone(), Arc::new(MemorySessionStore::default()));
        assert_eq!(
            client.router_wifi_clients().await.unwrap_err(),
            CloudError::NotConnected
        );
        assert_eq!(
            client.router_wifi_config().await.unwrap_err(),
            CloudError::NotConnected
        );
        assert!(transport.grpc_requests.lock().unwrap().is_empty());
        assert!(transport.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn router_network_replies_decode_expected_fields_and_reject_wrong_shapes() {
        let clients = serde_json::json!({"wifiGetClients": {"clients": [{"clientId": 42}]}});
        assert_eq!(
            router_wifi_clients_from(&clients).unwrap(),
            serde_json::json!([{"clientId": 42}])
        );
        assert_eq!(
            router_wifi_clients_from(&serde_json::json!({"wifiGetClients": {}})).unwrap(),
            serde_json::json!([])
        );
        assert!(matches!(
            router_wifi_clients_from(&serde_json::json!({"wifiGetClients": {"clients": {}}})),
            Err(CloudError::GrpcProtocol(_))
        ));

        let config = serde_json::json!({"wifiGetConfig": {"wifiConfig": {"countryCode": "US"}}});
        assert_eq!(
            router_wifi_config_from(&config).unwrap(),
            serde_json::json!({"countryCode": "US"})
        );
        assert_eq!(
            router_wifi_config_from(&serde_json::json!({"wifiGetConfig": {}})).unwrap(),
            serde_json::json!({})
        );
        assert!(matches!(
            router_wifi_config_from(&serde_json::json!({"wifiGetConfig": {"wifiConfig": []}})),
            Err(CloudError::GrpcProtocol(_))
        ));
    }

    // SPEC-CLOUDCFG-002: typed configuration writes replace their placeholder
    // with the fresh controller and traverse the same one-shot gateway.
    #[tokio::test]
    async fn router_config_write_targets_fresh_controller_and_is_schema_encoded() {
        let transport = Arc::new(FakeTransport::scripted_with_grpc(
            vec![
                refresh_response("access"),
                service_lines(),
                response(
                    200,
                    serde_json::json!({"data": {
                        "columnNamesByDeviceType": {
                            "r": ["kind", "DeviceId", "UtcTimestampNs", "WifiHopsFromController"]
                        },
                        "values": [["r", "Router-controller", 2_000_000_000, 0]]
                    }}),
                ),
            ],
            vec![grpc_response(200, encode_grpc_message(&[]))],
        ));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);

        client
            .update_router_config(router_config::RouterConfigUpdate::Bypass { enabled: true })
            .await
            .unwrap();

        let requests = transport.grpc_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let expected = client
            .schema
            .encode_request_json(&serde_json::json!({
                "targetId": "Router-controller",
                "wifiSetConfig": {"wifiConfig": {
                    "bypassMode": true,
                    "applyBypassMode": true
                }}
            }))
            .unwrap();
        assert_eq!(&requests[0].2[5..], expected);
    }

    // SPEC-DISHCFG-003: the cloud host selects the dish ID from fresh account
    // telemetry, then schema-encodes a single authenticated gateway write.
    #[tokio::test]
    async fn dish_config_write_uses_the_fresh_telemetry_target() {
        let transport = Arc::new(FakeTransport::scripted_with_grpc(
            vec![
                refresh_response("access"),
                service_lines(),
                response(
                    200,
                    serde_json::json!({"data": {
                        "columnNamesByDeviceType": {
                            "u": ["kind", "DeviceId", "UtcTimestampNs"]
                        },
                        "values": [["u", "ut-dish-123", 2_000_000_000]]
                    }}),
                ),
            ],
            vec![grpc_response(200, encode_grpc_message(&[]))],
        ));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);
        client
            .update_dish_config(dish_config::DishConfigUpdate {
                power_save_mode: Some(true),
                ..dish_config::DishConfigUpdate::default()
            })
            .await
            .unwrap();
        let requests = transport.grpc_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].0, DEVICE_HANDLE_URL);
        let expected = client
            .schema
            .encode_request_json(&serde_json::json!({
                "targetId": "ut-dish-123",
                "dishSetConfig": {"dishConfig": {
                    "powerSaveMode": true,
                    "applyPowerSaveMode": true
                }}
            }))
            .unwrap();
        assert_eq!(&requests[0].2[5..], expected);
    }

    // SPEC-DISHCFG-002: invalid values are refused before session lookup or
    // telemetry traffic.
    #[tokio::test]
    async fn invalid_dish_config_does_not_touch_the_cloud() {
        let transport = Arc::new(FakeTransport::default());
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);
        assert!(
            client
                .update_dish_config(dish_config::DishConfigUpdate::default())
                .await
                .is_err()
        );
        assert!(transport.requests.lock().unwrap().is_empty());
        assert!(transport.grpc_requests.lock().unwrap().is_empty());
    }

    // SPEC-CLOUDCFG-004: invalid subnet credentials are rejected before the
    // service-line, telemetry, or Wi-Fi context is requested.
    #[tokio::test]
    async fn invalid_subnet_write_does_not_touch_the_account_or_router() {
        let transport = Arc::new(FakeTransport::default());
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);
        assert!(matches!(
            client
                .update_router_config(router_config::RouterConfigUpdate::Subnet {
                    subnet: "192.168.2.1/24".into(),
                    password: "short".into(),
                })
                .await,
            Err(CloudError::GrpcProtocol(_))
        ));
        assert!(transport.requests.lock().unwrap().is_empty());
        assert!(transport.grpc_requests.lock().unwrap().is_empty());
    }

    // SPEC-CLOUDCFG-004: a valid subnet change reads the current Wi-Fi config
    // from the same freshly selected controller and refuses to wipe if it is
    // absent, without dispatching a set-config request.
    #[tokio::test]
    async fn subnet_write_reads_context_and_refuses_an_empty_network_list() {
        let transport = Arc::new(FakeTransport::scripted_with_grpc(
            vec![
                refresh_response("access"),
                service_lines(),
                response(
                    200,
                    serde_json::json!({"data": {
                        "columnNamesByDeviceType": {
                            "r": ["kind", "DeviceId", "UtcTimestampNs", "WifiHopsFromController"]
                        },
                        "values": [["r", "Router-controller", 2_000_000_000, 0]]
                    }}),
                ),
            ],
            vec![grpc_response(200, encode_grpc_message(&[]))],
        ));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);
        assert_eq!(
            client
                .update_router_config(router_config::RouterConfigUpdate::Subnet {
                    subnet: "192.168.2.1/24".into(),
                    password: "correct horse".into(),
                })
                .await,
            Err(CloudError::GrpcProtocol(
                "router reported no networks".into()
            ))
        );
        let requests = transport.grpc_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let expected = client
            .schema
            .encode_request_json(&serde_json::json!({
                "targetId": "Router-controller",
                "wifiGetConfig": {}
            }))
            .unwrap();
        assert_eq!(&requests[0].2[5..], expected);
    }

    // SPEC-CLOUDGW-002: a mesh-only telemetry snapshot never becomes a router
    // write target, even if it is the only router in that response.
    #[tokio::test]
    async fn router_gateway_refuses_a_mesh_node_as_controller() {
        let transport = Arc::new(FakeTransport::scripted_with_grpc(
            vec![
                refresh_response("access"),
                service_lines(),
                response(
                    200,
                    serde_json::json!({"data": {
                        "columnNamesByDeviceType": {
                            "r": ["kind", "DeviceId", "UtcTimestampNs", "WifiHopsFromController"]
                        },
                        "values": [["r", "Router-mesh", 2_000_000_000, 1]]
                    }}),
                ),
            ],
            vec![],
        ));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);

        assert_eq!(
            client
                .router_call(&serde_json::json!({"wifiGetConfig": {}}))
                .await,
            Err(CloudError::ControllerUnknown)
        );
        assert!(transport.grpc_requests.lock().unwrap().is_empty());
    }

    // SPEC-CLOUDGW-005, SPEC-CLOUDCFG-002: once a write receives an
    // unreachable reply, the gateway returns that outcome and does not replay it.
    #[tokio::test]
    async fn gateway_does_not_replay_a_device_unreachable_call() {
        let trailers = b"grpc-status:5\r\ngrpc-message:router%20offline\r\n";
        let mut response_body = vec![0x80];
        response_body.extend_from_slice(&(trailers.len() as u32).to_be_bytes());
        response_body.extend_from_slice(trailers);
        let transport = Arc::new(FakeTransport::scripted_with_grpc(
            vec![
                refresh_response("access"),
                service_lines(),
                response(
                    200,
                    serde_json::json!({"data": {
                        "columnNamesByDeviceType": {
                            "r": ["kind", "DeviceId", "UtcTimestampNs", "WifiHopsFromController"]
                        },
                        "values": [["r", "Router-controller", 2_000_000_000, 0]]
                    }}),
                ),
            ],
            vec![grpc_response(200, response_body)],
        ));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);

        assert_eq!(
            client
                .router_call(&serde_json::json!({"wifiGetConfig": {}}))
                .await,
            Err(CloudError::DeviceUnreachable)
        );
        assert_eq!(transport.grpc_requests.lock().unwrap().len(), 1);
    }

    // SPEC-CLOUDGW-005: an explicit unauthenticated gRPC status permits one
    // refresh and one retry; the second dispatch uses the renewed cookie.
    #[tokio::test]
    async fn gateway_refreshes_and_retries_once_after_grpc_unauthenticated() {
        let controller = response(
            200,
            serde_json::json!({"data": {
                "columnNamesByDeviceType": {
                    "r": ["kind", "DeviceId", "UtcTimestampNs", "WifiHopsFromController"]
                },
                "values": [["r", "Router-controller", 2_000_000_000, 0]]
            }}),
        );
        let transport = Arc::new(FakeTransport::scripted_with_grpc(
            vec![
                refresh_response("first"),
                service_lines(),
                controller.clone(),
                refresh_response("renewed"),
                controller,
            ],
            vec![
                GrpcHttpResponse {
                    status: 200,
                    grpc_status: Some(16),
                    grpc_message: Some("unauthenticated".into()),
                    body: vec![],
                },
                grpc_response(200, encode_grpc_message(&[])),
            ],
        ));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);

        assert!(
            client
                .router_call(&serde_json::json!({"wifiGetConfig": {}}))
                .await
                .is_ok()
        );
        let requests = transport.grpc_requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].1.contains("Access.V1=first"));
        assert!(requests[1].1.contains("Access.V1=renewed"));
    }

    // SPEC-CLOUDGW-003, SPEC-CLOUDGW-005: status and malformed-frame outcomes
    // are distinguishable, with no automatic replay of a dispatched call.
    #[test]
    fn grpc_statuses_and_frame_errors_are_classified_without_replay() {
        assert_eq!(
            device_grpc_error(5, String::new()),
            CloudError::DeviceUnreachable
        );
        assert_eq!(
            device_grpc_error(16, String::new()),
            CloudError::SessionExpired
        );
        assert!(matches!(
            device_grpc_error(7, "permission denied".into()),
            CloudError::GrpcStatus { code: 7, .. }
        ));
        assert!(matches!(
            decode_grpc_response(&[0, 0, 0]),
            Err(CloudError::GrpcProtocol(_))
        ));
        let mut trailers = encode_grpc_message(&[]);
        let trailer_bytes = b"grpc-status:7\r\ngrpc-message:permission%20denied\r\n";
        trailers.push(0x80);
        trailers.extend_from_slice(&(trailer_bytes.len() as u32).to_be_bytes());
        trailers.extend_from_slice(trailer_bytes);
        assert_eq!(
            decode_grpc_response(&trailers),
            Err(CloudError::GrpcStatus {
                code: 7,
                message: "permission denied".into()
            })
        );
    }

    // SPEC-CLOUD-003: Starlink's columnar telemetry is projected to stable
    // device IDs and the UI-facing typed field names.
    #[test]
    fn columnar_device_telemetry_is_projected_and_malformed_rows_are_skipped() {
        let projected = device_telemetry_from(&serde_json::json!({
            "data": {
                "columnNamesByDeviceType": {
                    "u": ["kind", "DeviceId", "UtcTimestampNs", "RunningSoftwareVersion", "Uptime", "ObstructionPercentTime", "SignalQuality"],
                    "r": ["kind", "DeviceId", "UtcTimestampNs", "WifiHardwareVersion", "WifiSoftwareVersion", "WifiUptimeS", "Clients", "WifiHopsFromController", "WifiIsRepeater", "WifiIsBypassed"]
                },
                "values": [
                    ["u", "ut-dish-123", 1_234_000_000, "v2026.01", 99, 0.15, 0.92],
                    ["r", "Router-aabb", "2000000000", "v3", "v1.2", 50, 4, 1, true, false],
                    ["unknown", "device-ignored", 0]
                ]
            }
        }));
        assert_eq!(projected["ut-dish-123"]["kind"], "dish");
        assert_eq!(projected["ut-dish-123"]["timestampMs"], 1234.0);
        assert_eq!(projected["ut-dish-123"]["obstructionPct"], 0.15);
        assert_eq!(projected["Router-aabb"]["kind"], "router");
        assert_eq!(projected["Router-aabb"]["timestampMs"], 2000.0);
        assert_eq!(projected["Router-aabb"]["isRepeater"], true);
        assert!(projected.get("device-ignored").is_none());
    }

    #[test]
    fn session_cookie_is_not_required_to_construct_a_client() {
        let session = MemorySessionStore::default();
        assert_eq!(session.read_cookie().unwrap(), None);
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        assert_eq!(
            session.read_cookie().unwrap().as_deref(),
            Some("Starlink.Com.Sso=session")
        );
        session.clear_cookie().unwrap();
        assert_eq!(session.read_cookie().unwrap(), None);
    }

    // SPEC-CLOUD-001, SPEC-CLOUD-003, SPEC-CLOUD-005: account and annotated
    // usage use the authenticated service line, stay independent of LAN state,
    // and keep credentials out of results.
    #[tokio::test]
    async fn account_and_usage_reads_follow_the_cloud_contract() {
        let transport = Arc::new(FakeTransport::scripted(vec![
            refresh_response("access"),
            service_lines(),
            response(200, serde_json::json!({"email":"user@example.test"})),
            response(200, serde_json::json!({"content":{"nickname":"Home"}})),
            response(200, serde_json::json!({"data":{"values":[]}})),
            response(200, serde_json::json!({"cycles":[]})),
        ]));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);

        let account = client.account().await.unwrap();
        assert_eq!(account.identity.unwrap()["email"], "user@example.test");
        assert_eq!(account.service_line.unwrap()["content"]["nickname"], "Home");
        assert_eq!(account.device_telemetry, serde_json::json!({}));
        assert!(
            account
                .device_telemetry
                .to_string()
                .find("Access.V1")
                .is_none()
        );
        let usage = client.usage().await.unwrap();
        assert_eq!(usage["cycles"], serde_json::json!([]));

        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 6);
        assert_eq!(requests[1].0, format!("{API}{SERVICE_LINES_PATH}"));
        assert_eq!(
            requests[3].0,
            format!("{API}/webagg/v2/accounts/service-line/SL-1")
        );
        assert_eq!(
            requests[4].2.as_ref().unwrap(),
            &serde_json::json!({"accountNumber":"ACC-1"})
        );
        assert_eq!(
            requests[5].0,
            format!("{API}/telemetryagg/v1/data-usage/account/ACC-1/service-line/SL-1/annotated")
        );
        assert!(
            requests
                .iter()
                .all(|(_, cookie, _)| cookie.contains("Starlink.Com.Sso=session"))
        );
        assert!(
            requests
                .iter()
                .skip(1)
                .all(|(_, cookie, _)| cookie.contains("Access.V1=access"))
        );
    }

    // SPEC-CLOUD-002: authentication expiry from optional account data must
    // still reach the shared refresh-and-retry path.
    #[tokio::test]
    async fn account_retries_when_identity_read_reports_expired_session() {
        let transport = Arc::new(FakeTransport::scripted(vec![
            refresh_response("first"),
            service_lines(),
            response(401, serde_json::json!({})),
            refresh_response("second"),
            response(200, serde_json::json!({"email":"user@example.test"})),
            response(200, serde_json::json!({"content":{"nickname":"Home"}})),
            response(200, serde_json::json!({"data":{"values":[]}})),
        ]));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);

        let account = client.account().await.unwrap();

        assert_eq!(
            account.identity.as_ref().unwrap()["email"],
            "user@example.test"
        );
        let requests = transport.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|(url, cookie, _)| url == AUTH_URL && !cookie.contains("Access.V1="))
                .count(),
            2
        );
        assert!(requests[4].1.contains("Access.V1=second"));
    }

    // SPEC-CLOUD-002: optional telemetry may be absent on upstream failure,
    // but an expired session must not be converted into an empty telemetry map.
    #[tokio::test]
    async fn account_retries_when_telemetry_read_reports_expired_session() {
        let transport = Arc::new(FakeTransport::scripted(vec![
            refresh_response("first"),
            service_lines(),
            response(200, serde_json::json!({"email":"user@example.test"})),
            response(200, serde_json::json!({"content":{"nickname":"Home"}})),
            response(401, serde_json::json!({})),
            refresh_response("second"),
            response(200, serde_json::json!({"email":"user@example.test"})),
            response(200, serde_json::json!({"content":{"nickname":"Home"}})),
            response(
                200,
                serde_json::json!({"data": {
                    "columnNamesByDeviceType": {
                        "u": ["kind", "DeviceId", "UtcTimestampNs"]
                    },
                    "values": [["u", "ut-dish-1", 1_000_000_000]]
                }}),
            ),
        ]));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(transport.clone(), session);

        let account = client.account().await.unwrap();

        assert_eq!(account.device_telemetry["ut-dish-1"]["kind"], "dish");
        let requests = transport.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|(url, cookie, _)| url == AUTH_URL && !cookie.contains("Access.V1="))
                .count(),
            2
        );
        assert!(requests[8].1.contains("Access.V1=second"));
    }

    // SPEC-CLOUD-002: one rejected session refreshes once, then retries once.
    #[tokio::test]
    async fn expired_access_cookie_is_refreshed_and_retried_once() {
        let transport = Arc::new(FakeTransport::scripted(vec![
            refresh_response("first"),
            response(401, serde_json::json!({})),
            refresh_response("second"),
            service_lines(),
        ]));
        let session = Arc::new(MemorySessionStore::default());
        let client = CloudClient::new(transport.clone(), session.clone());
        client.connect("Starlink.Com.Sso=session").await.unwrap();

        let requests = transport.requests.lock().unwrap();
        assert_eq!(requests.len(), 4);
        assert_eq!(
            requests[1].1,
            "Starlink.Com.Access.V1=first;Starlink.Com.Sso=session"
        );
        assert_eq!(
            requests[3].1,
            "Starlink.Com.Access.V1=second;Starlink.Com.Sso=session"
        );
        assert!(
            session
                .read_cookie()
                .unwrap()
                .unwrap()
                .contains("Starlink.Com.Sso=session")
        );
    }

    // SPEC-CLOUD-009: a durable SSO cookie may precede the token that makes it
    // usable; retain and re-verify it before declaring sign-in unsuccessful.
    #[tokio::test]
    async fn connect_retries_a_session_cookie_that_is_not_ready_yet() {
        let transport = Arc::new(FakeTransport::scripted(vec![
            response(403, serde_json::json!({})),
            response(403, serde_json::json!({})),
            refresh_response("settled"),
            service_lines(),
        ]));
        let session = Arc::new(MemorySessionStore::default());
        let client = CloudClient::new(transport.clone(), session.clone());

        client.connect("Starlink.Com.Sso=session").await.unwrap();

        assert_eq!(transport.requests.lock().unwrap().len(), 4);
        assert!(
            session
                .read_cookie()
                .unwrap()
                .is_some_and(|cookie| cookie.contains("Starlink.Com.Sso=session"))
        );
    }

    // SPEC-CLOUD-002: auth and upstream failures have distinct error classes.
    #[tokio::test]
    async fn auth_expiry_and_upstream_failure_remain_distinct() {
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let expired = CloudClient::new(
            Arc::new(FakeTransport::scripted(vec![
                response(403, serde_json::json!({})),
                response(401, serde_json::json!({})),
            ])),
            session.clone(),
        );
        assert_eq!(expired.usage().await, Err(CloudError::SessionExpired));

        let upstream = CloudClient::new(
            Arc::new(FakeTransport::scripted(vec![
                refresh_response("access"),
                response(502, serde_json::json!({})),
            ])),
            session,
        );
        assert_eq!(upstream.usage().await, Err(CloudError::Http(502)));
    }

    // SPEC-CLOUD-004: only the host store receives and retains the credential.
    #[tokio::test]
    async fn disconnect_clears_the_host_session() {
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = CloudClient::new(Arc::new(FakeTransport::default()), session.clone());
        client.disconnect().unwrap();
        assert!(!client.is_connected().unwrap());
        assert_eq!(session.read_cookie().unwrap(), None);
    }

    // SPEC-CLOUD-004: a browser session refused by the Starlink auth endpoint
    // must not be retained as if it were connected.
    #[tokio::test]
    async fn rejected_session_is_removed_from_the_host_store() {
        let session = Arc::new(MemorySessionStore::default());
        let client = CloudClient::new(
            Arc::new(FakeTransport::scripted(vec![
                response(
                    403,
                    serde_json::json!({})
                );
                CONNECT_ATTEMPTS * 2
            ])),
            session.clone(),
        );
        assert_eq!(
            client.connect("Starlink.Com.Sso=invalid").await,
            Err(CloudError::SessionExpired)
        );
        assert_eq!(session.read_cookie().unwrap(), None);
    }

    // SPEC-CLOUD-001: simultaneous reads share a single token refresh.
    #[tokio::test]
    async fn simultaneous_reads_share_the_access_cookie_refresh() {
        let transport = Arc::new(FakeTransport::scripted(vec![
            refresh_response("shared"),
            service_lines(),
            service_lines(),
            response(200, serde_json::json!({"cycles":[]})),
            response(200, serde_json::json!({"cycles":[]})),
        ]));
        let session = Arc::new(MemorySessionStore::default());
        session.write_cookie("Starlink.Com.Sso=session").unwrap();
        let client = Arc::new(CloudClient::new(transport.clone(), session));
        let (first, second) = tokio::join!(client.usage(), client.usage());
        first.unwrap();
        second.unwrap();

        let requests = transport.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|(url, _, _)| url == AUTH_URL)
                .count(),
            1
        );
    }
}

#[cfg(test)]
mod router_config_tests {
    use super::router_config::{
        RouterConfigUpdate, build_router_config_request, build_router_config_request_with_context,
    };

    // SPEC-CLOUDCFG-001: DNS addresses are canonicalized, de-duplicated, and
    // an empty list explicitly restores default resolvers.
    #[test]
    fn spec_cloudcfg_001_dns_is_normalized_and_deduplicated() {
        let request = build_router_config_request(
            "Router-controller",
            RouterConfigUpdate::CustomDns {
                nameservers: vec![
                    "1.1.1.1".into(),
                    "2606:4700:4700::1111".into(),
                    "1.1.1.1".into(),
                ],
            },
        )
        .unwrap();
        assert_eq!(
            request,
            serde_json::json!({
                "targetId": "Router-controller",
                "wifiSetConfig": {"wifiConfig": {
                    "nameservers": ["1.1.1.1", "2606:4700:4700::1111"],
                    "applyNameservers": true
                }}
            })
        );
        let cleared = build_router_config_request(
            "Router-controller",
            RouterConfigUpdate::CustomDns {
                nameservers: vec![],
            },
        )
        .unwrap();
        assert_eq!(
            cleared["wifiSetConfig"]["wifiConfig"]["nameservers"],
            serde_json::json!([])
        );
    }

    // SPEC-CLOUDCFG-001: invalid literals and a fifth DNS entry are refused.
    #[test]
    fn spec_cloudcfg_001_dns_refuses_invalid_addresses_or_too_many_entries() {
        for nameservers in [vec!["dns.example".into()], vec!["1.1.1.1".into(); 5]] {
            assert!(
                build_router_config_request(
                    "Router-controller",
                    RouterConfigUpdate::CustomDns { nameservers },
                )
                .is_err()
            );
        }
    }

    // SPEC-CLOUDCFG-002: bypass and reset use distinct request oneofs.
    #[test]
    fn spec_cloudcfg_002_writes_use_the_expected_oneof_and_apply_flag() {
        let bypass = build_router_config_request(
            "Router-controller",
            RouterConfigUpdate::Bypass { enabled: true },
        )
        .unwrap();
        assert_eq!(
            bypass,
            serde_json::json!({
                "targetId": "Router-controller",
                "wifiSetConfig": {"wifiConfig": {
                    "bypassMode": true,
                    "applyBypassMode": true
                }}
            })
        );
        let reset =
            build_router_config_request("Router-controller", RouterConfigUpdate::FactoryReset)
                .unwrap();
        assert_eq!(
            reset,
            serde_json::json!({
                "targetId": "Router-controller",
                "factoryReset": {}
            })
        );
    }

    // SPEC-CLOUDCFG-004: subnet writes preserve the router's Wi-Fi config while
    // removing read-only IDs and replacing masked Wi-Fi passphrases.
    #[test]
    fn subnet_write_preserves_networks_and_cleans_router_assigned_fields() {
        let wifi_config = serde_json::json!({"networks": [
            {"ipv4":"192.168.1.1/24", "isGuest":false, "basicServiceSets":[
                {"ssid":"StarL X", "band":"RF_2GHZ", "bssid":"aa:bb", "ifaceName":"wlan0", "authWpa2":{"password":"•••••"}, "authRadius":{"password":"radius-password"}},
                {"ssid":"StarL X", "band":"RF_5GHZ", "bssid":"aa:bc", "ifaceName":"wlan1", "authWpa2":{"password":"•••••"}}
            ]},
            {"isGuest":true, "basicServiceSets": []}
        ]});
        let request = build_router_config_request_with_context(
            "Router-controller",
            RouterConfigUpdate::Subnet {
                subnet: "192.168.2.1/24".into(),
                password: "correct horse".into(),
            },
            &wifi_config,
        )
        .unwrap();
        let networks = request
            .pointer("/wifiSetConfig/wifiConfig/networks")
            .unwrap();
        assert_eq!(networks[0]["ipv4"], "192.168.2.1/24");
        assert_eq!(networks[1].get("ipv4"), None);
        assert_eq!(networks[0]["isGuest"], false);
        assert_eq!(
            networks[0]["basicServiceSets"][0]["authWpa2"]["password"],
            "correct horse"
        );
        assert_eq!(
            networks[0]["basicServiceSets"][0]["authRadius"]["password"],
            "radius-password"
        );
        assert_eq!(
            networks[0]["basicServiceSets"][1]["authWpa2"]["password"],
            "correct horse"
        );
        assert_eq!(networks[0]["basicServiceSets"][0].get("bssid"), None);
        assert_eq!(networks[0]["basicServiceSets"][0].get("ifaceName"), None);
        assert_eq!(
            request["wifiSetConfig"]["wifiConfig"]["applyNetworks"],
            true
        );
    }

    // SPEC-CLOUDCFG-005: mesh-name writes change one known name and strip the
    // router-assigned incarnation from the complete replacement map.
    #[test]
    fn mesh_name_write_preserves_other_fields_and_strips_incarnations() {
        let wifi_config = serde_json::json!({"meshConfigs": {
            "Router-node1": {"displayName":"Mesh 1", "auth":"MESH_AUTH_TRUSTED", "incarnation":"123"},
            "Router-node2": {"displayName":"Mesh 2", "auth":"MESH_AUTH_TRUSTED", "incarnation":"456"}
        }});
        let request = build_router_config_request_with_context(
            "Router-controller",
            RouterConfigUpdate::MeshName {
                device_id: "Router-node1".into(),
                display_name: "  Garage  ".into(),
            },
            &wifi_config,
        )
        .unwrap();
        let configs = &request["wifiSetConfig"]["wifiConfig"]["meshConfigs"];
        assert_eq!(configs["Router-node1"]["displayName"], "Garage");
        assert_eq!(configs["Router-node1"]["auth"], "MESH_AUTH_TRUSTED");
        assert_eq!(configs["Router-node2"]["displayName"], "Mesh 2");
        assert_eq!(configs["Router-node1"].get("incarnation"), None);
        assert_eq!(configs["Router-node2"].get("incarnation"), None);
        assert_eq!(
            request["wifiSetConfig"]["wifiConfig"]["applyMeshConfigs"],
            true
        );
    }
}
