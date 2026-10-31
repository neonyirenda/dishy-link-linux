//! Runtime access to the dish's protobuf schema.
//!
//! The schema is not written by hand here. [`BUNDLED_PROTOSET`] is the descriptor
//! set dumped from the dish's own gRPC reflection service, so every field number
//! and wire type below either exists in that file or fails to decode. This is the
//! same contract the TypeScript client uses (upstream `core/dishClient.ts`).
//!
//! Requests are oneof selections with an empty or tiny sub-message, so they are
//! hand-encoded by [`encode_oneof_request`]. Responses are decoded dynamically,
//! exactly as the existing client does, because firmware updates add fields that
//! a compiled-in struct would silently drop.

use prost::Message as _;
use prost_reflect::{
    DescriptorError, DescriptorPool, DeserializeOptions, DynamicMessage, MessageDescriptor,
};
use serde_json::Value;

pub mod fields;

/// The descriptor set captured from the dish's gRPC reflection service
/// (`dish.protoset` in this crate).
///
/// Embedded rather than read at runtime so a packaged app has one less file to
/// find. Hosts that need to follow a firmware update can build a [`Schema`] from
/// fresh bytes with [`Schema::from_bytes`].
pub const BUNDLED_PROTOSET: &[u8] = include_bytes!("../dish.protoset");

/// Both the dish and the router answer this same service.
pub const REQUEST_MESSAGE: &str = "SpaceX.API.Device.Request";
pub const RESPONSE_MESSAGE: &str = "SpaceX.API.Device.Response";

#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("descriptor set could not be decoded: {0}")]
    Descriptor(#[from] DescriptorError),
    #[error("descriptor set does not define {0}")]
    MissingMessage(&'static str),
    #[error("protobuf decode failed: {0}")]
    Decode(#[from] prost::DecodeError),
    #[error("JSON conversion failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// A loaded request/response pair, ready to encode requests and decode replies.
#[derive(Clone)]
pub struct Schema {
    pool: DescriptorPool,
    request: MessageDescriptor,
    response: MessageDescriptor,
}

impl Schema {
    /// Load the descriptor set shipped with the app.
    pub fn bundled() -> Result<Self, SchemaError> {
        Self::from_bytes(BUNDLED_PROTOSET)
    }

    /// Load a descriptor set captured from a dish's reflection service.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SchemaError> {
        let pool = DescriptorPool::decode(bytes)?;
        let request = pool
            .get_message_by_name(REQUEST_MESSAGE)
            .ok_or(SchemaError::MissingMessage(REQUEST_MESSAGE))?;
        let response = pool
            .get_message_by_name(RESPONSE_MESSAGE)
            .ok_or(SchemaError::MissingMessage(RESPONSE_MESSAGE))?;
        Ok(Self {
            pool,
            request,
            response,
        })
    }

    /// Encode a oneof selection, e.g. `get_status` with an empty sub-message.
    ///
    /// Returns the bytes of a `Device.Request`, ready to be framed for grpc-web.
    pub fn encode_oneof_request(&self, field_number: u32, sub_message: &[u8]) -> Vec<u8> {
        encode_oneof_request(field_number, sub_message)
    }

    /// Encode a full request from proto3 JSON. Used by config writes and by the
    /// cloud gateway, which needs payloads this client never hand-encodes.
    pub fn encode_request_json(&self, value: &Value) -> Result<Vec<u8>, SchemaError> {
        let text = serde_json::to_string(value)?;
        let mut deserializer = serde_json::Deserializer::from_str(&text);
        let message = DynamicMessage::deserialize_with_options(
            self.request.clone(),
            &mut deserializer,
            &DeserializeOptions::new().deny_unknown_fields(false),
        )?;
        deserializer.end()?;
        Ok(message.encode_to_vec())
    }

    /// Decode a `Device.Response` into dynamic protobuf. Typed accessors live in
    /// `dishy-core`, which owns the field shapes the app actually reads.
    pub fn decode_response(&self, bytes: &[u8]) -> Result<DynamicMessage, SchemaError> {
        Ok(DynamicMessage::decode(self.response.clone(), bytes)?)
    }

    /// Decode a `Device.Response` into proto3 JSON, the same representation the
    /// TypeScript client produces.
    pub fn decode_response_json(&self, bytes: &[u8]) -> Result<Value, SchemaError> {
        let message = self.decode_response(bytes)?;
        Ok(serde_json::to_value(&message)?)
    }

    /// The raw descriptor pool, for callers that need a message by name.
    pub fn pool(&self) -> &DescriptorPool {
        &self.pool
    }
}

/// Encode a `Device.Request` whose oneof selects `field_number`, with the given
/// sub-message bytes.
///
/// A oneof member is a message field, so the tag uses wire type 2 (length
/// delimited) and the length prefix is a varint, not a single byte. The
/// TypeScript client historically wrote a one-byte length; that is only correct
/// for sub-messages below 128 bytes. This implementation is correct for any size.
pub fn encode_oneof_request(field_number: u32, sub_message: &[u8]) -> Vec<u8> {
    const LENGTH_DELIMITED_WIRE_TYPE: u32 = 2;
    let mut out = Vec::with_capacity(10 + sub_message.len());
    encode_varint(
        u64::from((field_number << 3) | LENGTH_DELIMITED_WIRE_TYPE),
        &mut out,
    );
    encode_varint(sub_message.len() as u64, &mut out);
    out.extend_from_slice(sub_message);
    out
}

/// Protobuf base-128 varint.
pub fn encode_varint(mut value: u64, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

#[cfg(test)]
mod tests {
    use super::*;

    // SPEC-SCHEMA-001: the bundled descriptor set defines Request and Response.
    #[test]
    fn bundled_schema_defines_both_device_messages() {
        let schema = Schema::bundled().expect("bundled protoset");
        assert_eq!(schema.request.full_name(), REQUEST_MESSAGE);
        assert_eq!(schema.response.full_name(), RESPONSE_MESSAGE);
    }

    // SPEC-SCHEMA-002: a oneof request is tag + varint length + sub-message.
    #[test]
    fn oneof_request_uses_a_length_delimited_tag() {
        // 1004 << 3 | 2 = 8034 → varint e2 3e, then a zero length.
        assert_eq!(
            encode_oneof_request(fields::GET_STATUS, &[]),
            vec![0xE2, 0x3E, 0x00]
        );
        // 2002 << 3 | 2 = 16018 → 92 7d, then a two-byte body.
        assert_eq!(
            encode_oneof_request(fields::DISH_STOW, &[0x08, 0x01]),
            vec![0x92, 0x7D, 0x02, 0x08, 0x01]
        );
    }

    // SPEC-SCHEMA-002: sizes that need a multi-byte varint round-trip.
    #[test]
    fn oneof_request_handles_submessages_over_127_bytes() {
        let body = vec![0xAB; 300];
        let encoded = encode_oneof_request(fields::GET_DIAGNOSTICS, &body);
        assert_eq!(&encoded[..5], &[0x82, 0xF7, 0x02, 0xAC, 0x02]);
        assert_eq!(&encoded[5..], body.as_slice());
    }

    // SPEC-SCHEMA-004: proto3 JSON can be encoded back into the schema.
    #[test]
    fn json_encodes_to_protobuf() {
        let schema = Schema::bundled().expect("bundled protoset");
        let empty = serde_json::json!({ "getStatus": {} });
        let bytes = schema.encode_request_json(&empty).expect("encode");
        assert_eq!(bytes, encode_oneof_request(fields::GET_STATUS, &[]));
    }
}
