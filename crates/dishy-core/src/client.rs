//! Typed access to the dish and router APIs.
//!
//! The transport and schema are separate concerns; this module is the thin part
//! that selects a oneof, names the response branch, and returns JSON. Typed
//! telemetry shapes arrive with the telemetry port (spec 0020, milestone M2);
//! keeping this layer JSON-first until then avoids inventing a struct that has
//! to be changed the moment the real shape is ported.

use std::sync::Arc;

use dishy_proto::{Schema, SchemaError, fields};
use serde_json::Value;

use crate::control::ControlAction;
use crate::grpc_web::{GrpcWebClient, GrpcWebError};

/// The dish and the router answer the same service on different ports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Dish,
    Router,
}

impl Target {
    /// Where this box answers on its own LAN.
    ///
    /// The dish's 9201 is grpc-web; the router's 9001 is the matching endpoint.
    /// Router requests use the host's default gateway, falling back to the
    /// historical router address when no gateway route exists.
    pub fn default_handle_url(self) -> String {
        match self {
            Self::Dish => DISH_LAN_HANDLE_URL.to_owned(),
            Self::Router => {
                router_handle_url(&crate::gateway::default_router_address().to_string())
            }
        }
    }
}

pub const DISH_LAN_ADDRESS: &str = "192.168.100.1";
pub const ROUTER_LAN_ADDRESS: &str = "192.168.1.1";
pub const DISH_LAN_HANDLE_URL: &str = "http://192.168.100.1:9201/SpaceX.API.Device.Device/Handle";
pub const ROUTER_LAN_HANDLE_URL: &str = "http://192.168.1.1:9001/SpaceX.API.Device.Device/Handle";

pub fn router_handle_url(address: &str) -> String {
    let authority = if address.contains(':') {
        format!("[{address}]")
    } else {
        address.to_owned()
    };
    format!("http://{authority}:9001/SpaceX.API.Device.Device/Handle")
}

#[derive(Debug, thiserror::Error)]
pub enum DishClientError {
    #[error(transparent)]
    Transport(#[from] GrpcWebError),
    #[error(transparent)]
    Schema(#[from] SchemaError),
}

/// A client bound to one box. Cheap to clone: the schema and transport are
/// shared, which is what keeps concurrent pollers on one connection pool.
#[derive(Clone)]
pub struct DishClient {
    transport: Arc<GrpcWebClient>,
    schema: Arc<Schema>,
    handle_url: String,
}

impl DishClient {
    /// Build a client against a box's default LAN address.
    pub fn new(target: Target) -> Result<Self, DishClientError> {
        Ok(Self {
            transport: Arc::new(GrpcWebClient::new()),
            schema: Arc::new(Schema::bundled()?),
            handle_url: target.default_handle_url(),
        })
    }

    /// Build the router client at the host-configured address. The caller
    /// validates the literal before it reaches this transport boundary.
    pub fn router_at_address(address: &str) -> Result<Self, DishClientError> {
        Ok(Self {
            transport: Arc::new(GrpcWebClient::new()),
            schema: Arc::new(Schema::bundled()?),
            handle_url: router_handle_url(address),
        })
    }

    /// Build against an explicit URL — a router moved off its default address,
    /// or a fake server in a test.
    pub fn with_handle_url(
        transport: Arc<GrpcWebClient>,
        schema: Arc<Schema>,
        handle_url: impl Into<String>,
    ) -> Self {
        Self {
            transport,
            schema,
            handle_url: handle_url.into(),
        }
    }

    pub fn handle_url(&self) -> &str {
        &self.handle_url
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    /// Perform a oneof call and return the whole `Device.Response` as JSON.
    pub async fn call(
        &self,
        field_number: u32,
        sub_message: &[u8],
    ) -> Result<Value, DishClientError> {
        let request = self.schema.encode_oneof_request(field_number, sub_message);
        let response = self.transport.call(&self.handle_url, &request).await?;
        Ok(self.schema.decode_response_json(&response)?)
    }

    /// Perform a call and return one named branch of the response, or `{}` when
    /// the box answered without that branch. Mirrors the TypeScript accessors.
    async fn branch(&self, field_number: u32, branch: &str) -> Result<Value, DishClientError> {
        let response = self.call(field_number, &[]).await?;
        Ok(response
            .get(branch)
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default())))
    }

    pub async fn get_status(&self) -> Result<Value, DishClientError> {
        self.branch(fields::GET_STATUS, "dishGetStatus").await
    }

    /// The router's own status, from the same request field.
    pub async fn get_router_status(&self) -> Result<Value, DishClientError> {
        self.branch(fields::GET_STATUS, "wifiGetStatus").await
    }

    pub async fn get_history(&self) -> Result<Value, DishClientError> {
        self.branch(fields::GET_HISTORY, "dishGetHistory").await
    }

    /// The router's event log, answered to the same request on the router.
    pub async fn get_wifi_history(&self) -> Result<Value, DishClientError> {
        self.branch(fields::GET_HISTORY, "wifiGetHistory").await
    }

    pub async fn get_device_info(&self) -> Result<Value, DishClientError> {
        let response = self.call(fields::GET_DEVICE_INFO, &[]).await?;
        Ok(response
            .pointer("/getDeviceInfo/deviceInfo")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default())))
    }

    pub async fn get_obstruction_map(&self) -> Result<Value, DishClientError> {
        self.branch(fields::DISH_GET_OBSTRUCTION_MAP, "dishGetObstructionMap")
            .await
    }

    pub async fn get_location(&self) -> Result<Value, DishClientError> {
        self.branch(fields::GET_LOCATION, "getLocation").await
    }

    pub async fn get_wifi_clients(&self) -> Result<Value, DishClientError> {
        let response = self.call(fields::WIFI_GET_CLIENTS, &[]).await?;
        Ok(response
            .pointer("/wifiGetClients/clients")
            .cloned()
            .unwrap_or_else(|| Value::Array(Vec::new())))
    }

    pub async fn get_radio_stats(&self) -> Result<Value, DishClientError> {
        self.branch(fields::GET_RADIO_STATS, "getRadioStats").await
    }

    pub async fn get_config(&self) -> Result<Value, DishClientError> {
        let response = self.call(fields::DISH_GET_CONFIG, &[]).await?;
        Ok(response
            .pointer("/dishGetConfig/dishConfig")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default())))
    }

    pub async fn get_wifi_config(&self) -> Result<Value, DishClientError> {
        let response = self.call(fields::WIFI_GET_CONFIG, &[]).await?;
        Ok(response
            .pointer("/wifiGetConfig/wifiConfig")
            .cloned()
            .unwrap_or_else(|| Value::Object(Default::default())))
    }

    pub async fn get_diagnostics(&self) -> Result<Value, DishClientError> {
        self.branch(fields::GET_DIAGNOSTICS, "getDiagnostics").await
    }

    /// Send one confirmed maintenance write. The caller is responsible for
    /// picking the client that matches the action's target; the recorder thread
    /// is the only owner of either client.
    // SPEC-CTRL-002, SPEC-SAFETY-004: a one-shot write, never a poll.
    pub async fn perform_control(&self, action: ControlAction) -> Result<(), DishClientError> {
        self.call(action.field(), action.body()).await?;
        Ok(())
    }

    /// Encode a request for the cloud gateway, which cannot use this LAN path
    /// for every call. Kept here so both transports share one schema.
    pub fn encode_request_json(&self, value: &Value) -> Result<Vec<u8>, DishClientError> {
        Ok(self.schema.encode_request_json(value)?)
    }

    pub fn decode_response_json(&self, bytes: &[u8]) -> Result<Value, DishClientError> {
        Ok(self.schema.decode_response_json(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SCHEMA-001: the client loads the bundled schema without a file.
    #[test]
    fn default_clients_use_the_documented_ports() {
        assert!(DISH_LAN_HANDLE_URL.contains(":9201/"));
        assert!(ROUTER_LAN_HANDLE_URL.contains(":9001/"));
        assert_eq!(Target::Dish.default_handle_url(), DISH_LAN_HANDLE_URL);
        assert_eq!(
            Target::Router.default_handle_url(),
            router_handle_url(&crate::gateway::default_router_address().to_string())
        );
    }

    // SPEC-CTRL-007: a saved router address maps to the device port, with IPv6
    // literals bracketed for a valid URI authority.
    #[test]
    fn router_handle_url_uses_the_configured_address() {
        assert_eq!(
            router_handle_url("10.0.0.1"),
            "http://10.0.0.1:9001/SpaceX.API.Device.Device/Handle"
        );
        assert_eq!(
            router_handle_url("2001:db8::1"),
            "http://[2001:db8::1]:9001/SpaceX.API.Device.Device/Handle"
        );
    }
}
