#!/usr/bin/env bash
set -euo pipefail

cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --release

./target/release/asset-squeeze --version
./target/release/asset-squeeze optimize --help >/dev/null
./target/release/asset-squeeze doctor --help >/dev/null
