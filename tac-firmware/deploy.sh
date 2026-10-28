#!/usr/bin/env bash
# Usage: ./deploy.sh <user@host | /dev/tty...>
#   /dev/...: uploads over the controller's serial protocol (BAUD, default 115200); the Pi must already run --mode serial.
set -e
cd "$(dirname "$0")"
DEST=${1:?usage: ./deploy.sh <user@host | /dev/tty...>}
TARGET=aarch64-unknown-linux-musl
SSH_OPTS="-o ConnectTimeout=10"
BAUD=${BAUD:-115200}
FIRMWARE=target/$TARGET/release/tac-firmware
CONTROLLER=../hal/target/$TARGET/release/controller

reply() {
  # A subscription from another session may still be live: its events are not the reply.
  while IFS= read -r -t 120 line <&3 || { echo "no reply from the controller" >&2; exit 1; }; do
    [[ $line == "@ "* ]] || break
  done
  echo "  $line"
  if [[ $line == POST* || $line == GET* ]]; then
    echo "the Pi echoes what was sent: a console (getty) owns the port, not the controller" >&2
    exit 1
  fi
  [[ $line == "200 "* ]] || exit 1
}

send() {
  local len crc
  len=$(wc -c < "$2" | tr -d ' ')
  # cksum -o 3 is zlib's CRC-32, the same one the controller checks.
  crc=$(cksum -o 3 "$2" | cut -d' ' -f1)
  echo "uploading $1 ($len bytes)"
  printf 'POST /update?name=%s&crc=%s %s\n' "$1" "$crc" "$len" >&3
  cat "$2" >&3
  reply
}

cargo build --release --target $TARGET
cargo build --release --target $TARGET --manifest-path ../hal/Cargo.toml --bin controller

if [[ $DEST == /dev/* ]]; then
  # stty on the open fd: on macOS the tty config is lost when it is closed.
  exec 3<>"$DEST"
  stty "$BAUD" raw -echo clocal <&3
  # Terminate any half-written line on the other side and discard its replies, or they are read as ours.
  printf '\n' >&3
  while IFS= read -r -t 1 _ <&3; do :; done
  send tac-firmware "$FIRMWARE"
  send controller "$CONTROLLER"
  send service tac-firmware.service
  printf 'GET /restart\n' >&3
  reply
  echo "ok: tac-firmware restarted"
  exit
fi

echo "uploading to $DEST"
scp $SSH_OPTS "$FIRMWARE" "$CONTROLLER" tac-firmware.service "$DEST:/tmp/"
echo "installing (asks for sudo)"
ssh -t $SSH_OPTS "$DEST" '
  sudo install -m 755 /tmp/tac-firmware /tmp/controller /usr/local/bin/ &&
  sudo install -m 644 /tmp/tac-firmware.service /etc/systemd/system/ &&
  sudo systemctl daemon-reload &&
  sudo systemctl enable tac-firmware &&
  sudo systemctl restart tac-firmware &&
  rm /tmp/tac-firmware /tmp/controller /tmp/tac-firmware.service
'
echo "ok: tac-firmware restarted"
