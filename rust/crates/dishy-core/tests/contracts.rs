//! Contract tests: the captured bytes the TypeScript implementation produced
//! must decode to the same JSON here.
//!
//! `scripts/export-contracts.mts` regenerates these files from `schema/`. The
//! fixtures are committed so this suite runs without Node: a mismatch is a
//! behaviour change, not a tooling problem.

mod support;

use dishy_proto::{Schema, fields};

use support::assert_json_matches;

const STATUS_RESPONSE: &[u8] = include_bytes!("../../../../contracts/grpc/status.response.bin");
const HISTORY_RESPONSE: &[u8] = include_bytes!("../../../../contracts/grpc/history.response.bin");
const OBSTRUCTION_RESPONSE: &[u8] =
    include_bytes!("../../../../contracts/grpc/obstruction.response.bin");
const GET_STATUS_REQUEST: &[u8] =
    include_bytes!("../../../../contracts/grpc/get-status.request.bin");

const STATUS_GOLDEN: &str = include_str!("../../../../contracts/golden/status.json");
const HISTORY_GOLDEN: &str = include_str!("../../../../contracts/golden/history.json");
const OBSTRUCTION_GOLDEN: &str = include_str!("../../../../contracts/golden/obstruction.json");

// SPEC-SCHEMA-003: a captured Device.Response decodes to proto3 JSON.
// SPEC-CONTRACT-001: the golden corpus is what the app actually reads.
#[test]
fn captured_status_decodes_to_the_golden_json() {
    let schema = Schema::bundled().expect("schema");
    let decoded = schema
        .decode_response_json(STATUS_RESPONSE)
        .expect("decode");
    assert!(decoded.get("dishGetStatus").is_some());
    assert_json_matches(&decoded, STATUS_GOLDEN);
}

#[test]
fn captured_history_decodes_to_the_golden_json() {
    let schema = Schema::bundled().expect("schema");
    let decoded = schema
        .decode_response_json(HISTORY_RESPONSE)
        .expect("decode");
    assert_json_matches(&decoded, HISTORY_GOLDEN);
}

#[test]
fn captured_obstruction_map_decodes_to_the_golden_json() {
    let schema = Schema::bundled().expect("schema");
    let decoded = schema
        .decode_response_json(OBSTRUCTION_RESPONSE)
        .expect("decode");
    assert_json_matches(&decoded, OBSTRUCTION_GOLDEN);
}

// SPEC-CONTRACT-002: the hand-encoded request is byte-identical to the one the
// TypeScript exporter produced.
#[test]
fn get_status_request_matches_the_typescript_frame() {
    let schema = Schema::bundled().expect("schema");
    assert_eq!(
        schema.encode_oneof_request(fields::GET_STATUS, &[]),
        GET_STATUS_REQUEST
    );
}
