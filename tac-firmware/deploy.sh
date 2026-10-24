#!/usr/bin/env bash
# Uso: ./deploy.sh [user@host | /dev/tty...]    (defecto creator@172.20.10.6)
#   /dev/...: sube por el protocolo serie del controller (BAUD, defecto 115200); el Pi ya debe correr --mode serial.
set -e
cd "$(dirname "$0")"
DEST=${1:-creator@172.20.10.6}
TARGET=aarch64-unknown-linux-musl
SSH_OPTS="-o ConnectTimeout=10"
BAUD=${BAUD:-115200}
FIRMWARE=target/$TARGET/release/tac-firmware
CONTROLLER=../hal/target/$TARGET/release/controller

reply() {
  # Una suscripcion de otra sesion puede seguir viva: sus eventos no son la respuesta.
  while IFS= read -r -t 120 line <&3 || { echo "sin respuesta del controller" >&2; exit 1; }; do
    [[ $line == "@ "* ]] || break
  done
  echo "  $line"
  if [[ $line == POST* || $line == GET* ]]; then
    echo "el Pi devuelve lo enviado: en el puerto hay una consola (getty), no el controller" >&2
    exit 1
  fi
  [[ $line == "200 "* ]] || exit 1
}

send() {
  local len crc
  len=$(wc -c < "$2" | tr -d ' ')
  # cksum -o 3 es el CRC-32 de zlib, el mismo que comprueba el controller.
  crc=$(cksum -o 3 "$2" | cut -d' ' -f1)
  echo "subiendo $1 ($len bytes)"
  printf 'POST /update?name=%s&crc=%s %s\n' "$1" "$crc" "$len" >&3
  cat "$2" >&3
  reply
}

cargo build --release --target $TARGET
cargo build --release --target $TARGET --manifest-path ../hal/Cargo.toml --bin controller

if [[ $DEST == /dev/* ]]; then
  # stty sobre el fd abierto: en macOS la config del tty se pierde al cerrarlo.
  exec 3<>"$DEST"
  stty "$BAUD" raw -echo clocal <&3
  # Cierra cualquier linea a medias del otro lado y descarta sus respuestas: si no, se leen como las nuestras.
  printf '\n' >&3
  while IFS= read -r -t 1 _ <&3; do :; done
  send tac-firmware "$FIRMWARE"
  send controller "$CONTROLLER"
  send service tac-firmware.service
  printf 'GET /restart\n' >&3
  reply
  echo "ok: tac-firmware reiniciado"
  exit
fi

echo "subiendo a $DEST"
scp $SSH_OPTS "$FIRMWARE" "$CONTROLLER" tac-firmware.service "$DEST:/tmp/"
echo "instalando (pide sudo)"
ssh -t $SSH_OPTS "$DEST" '
  sudo install -m 755 /tmp/tac-firmware /tmp/controller /usr/local/bin/ &&
  sudo install -m 644 /tmp/tac-firmware.service /etc/systemd/system/ &&
  sudo systemctl daemon-reload &&
  sudo systemctl enable tac-firmware &&
  sudo systemctl restart tac-firmware &&
  rm /tmp/tac-firmware /tmp/controller /tmp/tac-firmware.service
'
echo "ok: tac-firmware reiniciado"
