//! Decode a captured `Device.Response` and print proto3 JSON.
//!
//! Diff its output against `crates/dishy-core/fixtures/golden/` when a
//! contract test fails.
//!
//! `cargo run -p dishy-proto --example decode -- crates/dishy-core/fixtures/grpc/status.response.bin`

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
