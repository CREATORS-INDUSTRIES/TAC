#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["smbus2>=0.4"]
# ///
"""Lee fader (pin 2 -> ADS1115 A0) por I2C e imprime posicion 0.0-1.0.

ADS1115 -> Pi: VDD 3.3V, GND, SDA GPIO2, SCL GPIO3, ADDR GND (0x48).
Fader: 1 -> 3.3V, 2 -> A0, 3 -> GND.
Uso: uv run fader.py [--hz N] [--raw]
"""
import argparse
import sys
import time

from smbus2 import SMBus

ADDR = 0x48
REG_CONV = 0x00
REG_CONFIG = 0x01
# A0 single-ended, +-4.096V, continuo, 860 SPS, comparador off
CONFIG = 0xC2E3
FS_VOLTS = 4.096
VREF = 3.3


def main() -> None:
    p = argparse.ArgumentParser()
    p.add_argument("--hz", type=float, default=20.0)
    p.add_argument("--raw", action="store_true", help="mostrar cuentas del ADC")
    args = p.parse_args()

    bus = SMBus(1)
    bus.write_i2c_block_data(ADDR, REG_CONFIG, [CONFIG >> 8, CONFIG & 0xFF])
    time.sleep(0.05)

    lo, hi = 1.0, 0.0
    print("Leyendo A0. Ctrl+C para salir.")
    try:
        while True:
            hi_b, lo_b = bus.read_i2c_block_data(ADDR, REG_CONV, 2)
            raw = (hi_b << 8) | lo_b
            if raw > 0x7FFF:
                raw -= 0x10000
            v = min(1.0, max(0.0, raw * FS_VOLTS / 32768 / VREF))
            lo, hi = min(lo, v), max(hi, v)
            shown = f"{raw:6d}" if args.raw else f"{v:.3f}"
            bar = "#" * int(v * 40)
            sys.stdout.write(f"\r{shown}  [{bar:<40}]  min={lo:.3f} max={hi:.3f}")
            sys.stdout.flush()
            time.sleep(1.0 / args.hz)
    except KeyboardInterrupt:
        print(f"\nRango observado: {lo:.3f} - {hi:.3f}")
        bus.close()


if __name__ == "__main__":
    main()