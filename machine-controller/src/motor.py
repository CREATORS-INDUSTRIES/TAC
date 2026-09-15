#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["smbus2>=0.4", "gpiozero>=2.0", "lgpio>=0.2.2"]
# ///
"""Mueve el fader a una posicion 0.0-1.0 con bucle cerrado.

ADS1115 en I2C (ver fader.py). L298N: IN1 GPIO23, IN2 GPIO24, ENA GPIO18.
Uso: uv run motor.py 0.75       (una posicion)
     uv run motor.py            (interactivo, escribe posiciones)
"""
import os
import sys
import threading
import time

os.environ.setdefault("GPIOZERO_PIN_FACTORY", "lgpio")

from gpiozero import Motor
from smbus2 import SMBus

ADDR = 0x48
CONFIG = 0xC2E3
FS_VOLTS = 4.096
VREF = 3.3

# Ajusta con el rango que imprime fader.py al salir
POS_MIN = 0.005
POS_MAX = 0.985

KP = 6.0
DEADBAND = 0.008
MIN_PWM = 0.35
TIMEOUT = 1.5


class Fader:
    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.bus = SMBus(1)
        self.bus.write_i2c_block_data(ADDR, 0x01, [CONFIG >> 8, CONFIG & 0xFF])
        time.sleep(0.05)
        self.motor = Motor(forward=23, backward=24, enable=18)
        self.motor.forward_device.frequency = 10000
        self.motor.backward_device.frequency = 10000

    def read(self) -> float:
        with self.lock:
            hi, lo = self.bus.read_i2c_block_data(ADDR, 0x00, 2)
        raw = (hi << 8) | lo
        if raw > 0x7FFF:
            raw -= 0x10000
        v = raw * FS_VOLTS / 32768 / VREF
        return min(1.0, max(0.0, (v - POS_MIN) / (POS_MAX - POS_MIN)))

    def move_to(self, target: float) -> float:
        target = min(1.0, max(0.0, target))
        t0 = time.monotonic()
        while time.monotonic() - t0 < TIMEOUT:
            err = target - self.read()
            if abs(err) < DEADBAND:
                break
            speed = min(1.0, max(MIN_PWM, abs(err) * KP))
            if err > 0:
                self.motor.forward(speed)
            else:
                self.motor.backward(speed)
            time.sleep(0.002)
        self.motor.stop()
        return self.read()

    def close(self) -> None:
        self.motor.stop()
        self.motor.close()
        self.bus.close()


def main() -> None:
    f = Fader()
    try:
        if len(sys.argv) > 1:
            print(f"llegó a {f.move_to(float(sys.argv[1])):.3f}")
            return
        print("posición actual:", f"{f.read():.3f}", "| escribe 0.0-1.0, q para salir")
        while True:
            s = input("> ").strip()
            if s in ("q", ""):
                break
            print(f"llegó a {f.move_to(float(s)):.3f}")
    except KeyboardInterrupt:
        pass
    finally:
        f.close()


if __name__ == "__main__":
    main()
