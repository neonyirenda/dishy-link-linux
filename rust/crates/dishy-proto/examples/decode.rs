//! Decode a captured `Device.Response` and print proto3 JSON.
//!
//! Companion to the TypeScript `scripts/debug-decode.mjs`: same bytes, same
//! JSON, so a contract mismatch can be diffed instead of stared at.
//!
//! `cargo run -p dishy-proto --example decode -- contracts/grpc/status.response.bin`

use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path: PathBuf = std::env::args()
        .nth(1)
        .ok_or("usage: decode <response.bin>")?
        .into();
    let bytes = std::fs::read(path)?;
    let schema = dishy_proto::Schema::bundled()?;
    let json = schema.decode_response_json(&bytes)?;
    println!("{}", serde_json::to_string_pretty(&json)?);
    Ok(())
}
