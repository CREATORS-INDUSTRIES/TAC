#!/bin/sh
set -e
cd "$(dirname "$0")"
cargo build --release
cargo build --release --manifest-path ../hal/Cargo.toml --bin controller
sudo install -m 755 target/release/tac-firmware /usr/local/bin/tac-firmware
sudo install -m 755 ../hal/target/release/controller /usr/local/bin/controller
sudo install -m 644 tac-firmware.service /etc/systemd/system/tac-firmware.service
sudo systemctl daemon-reload
sudo systemctl enable --now tac-firmware
