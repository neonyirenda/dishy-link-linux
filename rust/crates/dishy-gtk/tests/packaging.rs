//! The packaging files agree with the app and ask for no more than it uses
//! (spec 0171).

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

fn packaging() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../packaging/linux")
}

fn read(name: &str) -> String {
    fs::read_to_string(packaging().join(name)).unwrap_or_else(|error| panic!("{name}: {error}"))
}

// SPEC-PKG-001: one id across the entry, the metadata, the icon, and the
// GApplication, and the entry runs the dishy-gtk binary.
#[test]
fn desktop_entry_and_metadata_name_the_app_and_binary() {
    let id = dishy_gtk::paths::APP_ID;
    let entry = read(&format!("{id}.desktop"));
    assert!(
        entry.lines().any(|line| line == "Exec=dishy-gtk"),
        "{entry}"
    );
    assert!(entry.lines().any(|line| line == format!("Icon={id}")));
    assert!(
        entry
            .lines()
            .any(|line| line == format!("StartupWMClass={id}"))
    );

    let metainfo = read(&format!("{id}.metainfo.xml"));
    assert!(metainfo.contains(&format!("<id>{id}</id>")));
    assert!(metainfo.contains(&format!(
        "<launchable type=\"desktop-id\">{id}.desktop</launchable>"
    )));
    assert!(metainfo.contains("<binary>dishy-gtk</binary>"));
}

// SPEC-PKG-002 / SPEC-PKG-004: the manifest grants exactly the listed access,
// installs the icons, entry, metadata, and secret-tool, and builds offline.
#[test]
fn the_flatpak_manifest_grants_only_what_the_app_uses() {
    let id = dishy_gtk::paths::APP_ID;
    let manifest: serde_json::Value =
        serde_json::from_str(&read(&format!("{id}.json"))).expect("manifest is JSON");
    assert_eq!(manifest["id"], id);
    assert_eq!(manifest["command"], "dishy-gtk");
    let granted: BTreeSet<&str> = manifest["finish-args"]
        .as_array()
        .expect("finish-args")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let expected: BTreeSet<&str> = [
        "--share=network",
        "--share=ipc",
        "--socket=wayland",
        "--socket=fallback-x11",
        "--device=dri",
        "--talk-name=org.freedesktop.secrets",
        "--talk-name=org.kde.StatusNotifierWatcher",
        "--own-name=org.kde.StatusNotifierItem-*",
        "--talk-name=org.freedesktop.Notifications",
        "--filesystem=xdg-config/autostart:create",
        // SPEC-SHEXT-004: installing and enabling the Shell extension.
        "--filesystem=xdg-data/gnome-shell/extensions:create",
        "--talk-name=org.gnome.Shell.Extensions",
    ]
    .into_iter()
    .collect();
    assert_eq!(granted, expected);

    let modules = manifest["modules"].as_array().expect("modules");
    let names: Vec<&str> = modules
        .iter()
        .filter_map(|module| module["name"].as_str())
        .collect();
    assert!(
        names.contains(&"libsecret"),
        "secret-tool is built: {names:?}"
    );
    let app = modules
        .iter()
        .find(|module| module["name"] == "dishylink")
        .expect("app module");
    let commands = app["build-commands"].as_array().expect("build commands");
    let joined: String = commands
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("--offline"), "{joined}");
    for installed in [
        format!("/app/share/applications/{id}.desktop"),
        format!("/app/share/metainfo/{id}.metainfo.xml"),
        format!("/app/share/icons/hicolor/scalable/apps/{id}.svg"),
        format!("/app/share/icons/hicolor/512x512/apps/{id}.png"),
    ] {
        assert!(joined.contains(&installed), "installs {installed}");
    }
    assert!(
        app["sources"]
            .as_array()
            .expect("sources")
            .iter()
            .any(|source| source == "cargo-sources.json"),
        "vendored crates are a source"
    );
}

// SPEC-PKG-002: every registry crate in Cargo.lock has a vendored source, so
// the offline build cannot drift from the lockfile.
#[test]
fn every_locked_crate_is_vendored() {
    let lock =
        fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock"))
            .expect("Cargo.lock");
    let sources: serde_json::Value =
        serde_json::from_str(&read("cargo-sources.json")).expect("cargo sources");
    let vendored: BTreeSet<String> = sources
        .as_array()
        .expect("source list")
        .iter()
        .filter_map(|source| source["dest"].as_str())
        .filter_map(|dest| dest.strip_prefix("cargo/vendor/"))
        .map(str::to_owned)
        .collect();
    let mut missing = Vec::new();
    for package in lock.split("[[package]]").skip(1) {
        let field = |key: &str| {
            package
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{key} = \"")))
                .map(|value| value.trim_end_matches('"').to_owned())
        };
        if field("source").is_some_and(|source| source.starts_with("registry+")) {
            let crate_dir = format!("{}-{}", field("name").unwrap(), field("version").unwrap());
            if !vendored.contains(&crate_dir) {
                missing.push(crate_dir);
            }
        }
    }
    assert!(missing.is_empty(), "not vendored: {missing:?}");
}

// SPEC-PKG-005: CI builds the bundle from this manifest and validates the
// metadata whenever the Rust tree or the packaging files change.
#[test]
fn ci_builds_the_flatpak_and_validates_the_metadata() {
    let workflow = fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../.github/workflows/rust.yml"),
    )
    .expect("rust workflow");
    for needle in [
        "packaging/linux/**",
        "manifest-path: packaging/linux/com.dishylink.app.json",
        "desktop-file-validate packaging/linux/com.dishylink.app.desktop",
        "appstreamcli validate",
        "generate-cargo-sources.py",
    ] {
        assert!(workflow.contains(needle), "workflow is missing {needle}");
    }
}

// SPEC-PILL-003: the Shell extension targets GNOME 46-51, reads the same bus
// name, object path, and interface the app exports, and ships its icon.
#[test]
fn shell_extension_matches_the_exported_status() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../packaging/gnome-shell-extension/dishylink@dishylink.app");
    let metadata: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("metadata.json")).unwrap())
            .unwrap();
    assert_eq!(metadata["uuid"], "dishylink@dishylink.app");
    let versions: Vec<&str> = metadata["shell-version"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    for version in ["46", "47", "48", "49", "50", "51"] {
        assert!(versions.contains(&version), "shell {version}");
    }
    let script = std::fs::read_to_string(root.join("extension.js")).unwrap();
    assert!(script.contains(&format!("\"{}\"", dishy_gtk::paths::APP_ID)));
    assert!(script.contains(&format!("\"{}\"", dishy_gtk::status_bus::OBJECT_PATH)));
    assert!(script.contains(dishy_gtk::status_bus::INTERFACE));
    // The same properties the app's introspection declares (checked there).
    for (name, _) in dishy_gtk::status_bus::StatusProperties::default().entries() {
        assert!(
            script.contains(&format!("name=\"{name}\"")),
            "extension lacks {name}"
        );
    }
    assert!(root.join("icons/dishylink-symbolic.svg").exists());
}

// SPEC-NAME-001: the desktop entry and AppStream metadata name the app
// "Dishylink Linux", matching its wordmark.
#[test]
fn packaging_names_the_app_dishylink_linux() {
    assert!(
        read("com.dishylink.app.desktop")
            .lines()
            .any(|line| line == "Name=Dishylink Linux")
    );
    assert!(read("com.dishylink.app.metainfo.xml").contains("<name>Dishylink Linux</name>"));
}
