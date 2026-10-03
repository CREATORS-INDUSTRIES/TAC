#!/bin/sh
# Uso: ./deploy.sh [user@host]    (defecto creator@172.20.10.6)
set -e
cd "$(dirname "$0")"
HOST=${1:-creator@172.20.10.6}
TARGET=aarch64-unknown-linux-musl
SSH_OPTS="-o ConnectTimeout=10"

cargo build --release --target $TARGET
cargo build --release --target $TARGET --manifest-path ../hal/Cargo.toml --bin controller

echo "subiendo a $HOST"
scp $SSH_OPTS target/$TARGET/release/tac-firmware ../hal/target/$TARGET/release/controller tac-firmware.service "$HOST:/tmp/"
echo "instalando (pide sudo)"
ssh -t $SSH_OPTS "$HOST" '
  sudo install -m 755 /tmp/tac-firmware /tmp/controller /usr/local/bin/ &&
  sudo install -m 644 /tmp/tac-firmware.service /etc/systemd/system/ &&
  sudo systemctl daemon-reload &&
  sudo systemctl enable tac-firmware &&
  sudo systemctl restart tac-firmware &&
  rm /tmp/tac-firmware /tmp/controller /tmp/tac-firmware.service
'
echo "ok: tac-firmware reiniciado"
