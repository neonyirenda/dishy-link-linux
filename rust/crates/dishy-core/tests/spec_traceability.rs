//! Spec traceability and hardware-safety guards.
//!
//! Every requirement in an `active` spec must be cited by at least one Rust
//! test. That is the whole point of the IDs: a requirement without a test is a
//! promise, not a behaviour, and should not be marked active.
//!
//! This test also enforces the one invariant that is cheap to check and
//! expensive to forget: no Rust source may poll `get_ping`.
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../..")
        .canonicalize()
        .expect("repo root")
}

fn read_specs(root: &Path) -> Vec<(PathBuf, String)> {
    let mut specs = Vec::new();
    for entry in fs::read_dir(root.join("specs")).expect("specs directory") {
        let path = entry.expect("spec entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("spec text");
        specs.push((path, text));
    }
    specs
}

/// Every spec past draft is enforced: active, complete, and verified specs
/// must all keep a citing test. Draft and superseded specs are not.
fn is_active(text: &str) -> bool {
    text.lines().take(10).any(|line| {
        matches!(
            line.trim(),
            "status: active" | "status: complete" | "status: verified"
        )
    })
}

fn requirement_ids(text: &str) -> Vec<String> {
    let pattern = Regex::new(r"SPEC-[A-Z]+-[0-9]{3}").expect("requirement ID regex");
    pattern
        .find_iter(text)
        .map(|m| m.as_str().to_string())
        .collect()
}

fn rust_sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).expect("read source dir") {
        let path = entry.expect("source entry").path();
        if path.is_dir() {
            rust_sources(&path, out);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            out.push(path);
        }
    }
}

// SPEC-SPEC-001 is intentionally not defined: the traceability rules are this
// test's own contract, described in `specs/README.md`.
#[test]
fn every_active_requirement_is_cited_by_a_rust_test() {
    let root = repo_root();
    let mut sources = Vec::new();
    rust_sources(&root.join("rust/crates"), &mut sources);
    let cited: String = sources
        .iter()
        .map(|path| fs::read_to_string(path).expect("source text"))
        .collect::<Vec<_>>()
        .join("\n");

    let mut missing = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for (path, text) in read_specs(&root) {
        if !is_active(&text) {
            continue;
        }
        for id in requirement_ids(&text) {
            if !seen.insert(id.clone()) {
                panic!(
                    "duplicate requirement ID {id} (second sighting in {})",
                    path.display()
                );
            }
            if !cited.contains(&id) {
                missing.push(id);
            }
        }
    }

    assert!(
        missing.is_empty(),
        "active requirements with no citing test: {missing:?}"
    );
}

// SPEC-SAFETY-002, SPEC-CTRL-002: the GUI asks the recorder thread for every
// device read and write. It must not link the transport or build a client.
#[test]
fn the_gui_crate_holds_no_device_client() {
    let root = repo_root();
    let mut sources = Vec::new();
    rust_sources(&root.join("rust/crates/dishy-gtk"), &mut sources);

    let mut offenders = Vec::new();
    for path in sources {
        let text = fs::read_to_string(&path).expect("source text");
        if text.contains("DishClient") || text.contains("GrpcWebClient") {
            offenders.push(path);
        }
    }

    assert!(
        offenders.is_empty(),
        "GUI source references a device client directly: {offenders:?}"
    );
}

// SPEC-SAFETY-001: `get_ping` is never called. It has rebooted the router on
// every trial; ping success comes from the `get_status` reply we already fetch.
#[test]
fn no_rust_source_calls_get_ping() {
    let root = repo_root();
    let mut sources = Vec::new();
    rust_sources(&root.join("rust/crates"), &mut sources);

    let mut offenders = Vec::new();
    for path in sources {
        // This file names the forbidden RPC in order to forbid it.
        if path.file_name().and_then(|name| name.to_str()) == Some("spec_traceability.rs") {
            continue;
        }
        let text = fs::read_to_string(&path).expect("source text");
        if text.to_ascii_lowercase().contains("get_ping") {
            offenders.push(path);
        }
    }

    assert!(
        offenders.is_empty(),
        "forbidden get_ping reference in: {offenders:?}"
    );
}

// SPEC-ARCH-002: the GUI crate makes no HTTP request of its own. Device, cloud,
// and public-internet reads go through `dishy-core` or `dishy-cloud`.
#[test]
fn the_gui_crate_has_no_http_client() {
    let root = repo_root();
    let crate_dir = root.join("rust/crates/dishy-gtk");
    let mut sources = Vec::new();
    rust_sources(&crate_dir, &mut sources);
    sources.push(crate_dir.join("Cargo.toml"));

    let offenders: Vec<_> = sources
        .into_iter()
        .filter(|path| {
            fs::read_to_string(path)
                .expect("source text")
                .contains("reqwest")
        })
        .collect();
    assert!(
        offenders.is_empty(),
        "GUI crate names reqwest directly: {offenders:?}"
    );
}
