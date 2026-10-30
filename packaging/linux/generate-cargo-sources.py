#!/usr/bin/env python3
"""Write cargo-sources.json for the Flatpak build from rust/Cargo.lock.

Same output shape as flatpak-builder-tools' flatpak-cargo-generator, without
its dependencies: each registry crate becomes a checksummed archive unpacked
under cargo/vendor, with the .cargo-checksum.json Cargo expects, plus a cargo
config that points crates.io at that directory. Re-run it whenever
Cargo.lock changes; the dishy-gtk packaging test fails until you do.

    python3 packaging/linux/generate-cargo-sources.py
"""

import json
import tomllib
from pathlib import Path

HERE = Path(__file__).resolve().parent
LOCK = HERE.parent.parent / "rust" / "Cargo.lock"
OUT = HERE / "cargo-sources.json"
REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"

CARGO_CONFIG = """[source.vendored-sources]
directory = "cargo/vendor"

[source.crates-io]
replace-with = "vendored-sources"
"""


def main() -> None:
    lock = tomllib.loads(LOCK.read_text())
    sources = []
    for package in lock["package"]:
        if package.get("source") != REGISTRY:
            continue
        name, version, checksum = package["name"], package["version"], package["checksum"]
        dest = f"cargo/vendor/{name}-{version}"
        sources.append(
            {
                "type": "archive",
                "archive-type": "tar-gzip",
                "url": f"https://static.crates.io/crates/{name}/{name}-{version}.crate",
                "sha256": checksum,
                "dest": dest,
            }
        )
        sources.append(
            {
                "type": "inline",
                "contents": json.dumps({"package": checksum, "files": {}}),
                "dest": dest,
                "dest-filename": ".cargo-checksum.json",
            }
        )
    sources.append(
        {"type": "inline", "contents": CARGO_CONFIG, "dest": "cargo", "dest-filename": "config.toml"}
    )
    OUT.write_text(json.dumps(sources, indent=2) + "\n")
    print(f"wrote {len(sources) // 2} crates to {OUT.relative_to(HERE.parent.parent)}")


if __name__ == "__main__":
    main()
