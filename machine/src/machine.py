#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["smbus2>=0.4", "gpiozero>=2.0", "lgpio>=0.2.2"]
# ///
"""Barra en vivo con la posicion del fader + prompt para mandarlo a 0-100.

Uso: uv run machine.py
"""
import sys
import threading
import time

from motor import Fader

WIDTH = 40
HZ = 20


def bar_line(pos: float) -> str:
    filled = int(pos * WIDTH)
    return f"[{'#' * filled}{'-' * (WIDTH - filled)}] {pos * 100:5.1f}%"


def display_loop(f: Fader, stop: threading.Event) -> None:
    # Escribe siempre en la linea encima del cursor sin moverlo: el prompt vive debajo.
    while not stop.is_set():
        sys.stdout.write("\0337\033[1A\r\033[2K" + bar_line(f.read()) + "\0338")
        sys.stdout.flush()
        time.sleep(1.0 / HZ)


def main() -> None:
    f = Fader()
    stop = threading.Event()
    t = threading.Thread(target=display_loop, args=(f, stop), daemon=True)
    print("Escribe 0-100 y Enter para mover. q para salir.")
    print()
    t.start()
    try:
        while True:
            s = input("> ").strip().lower()
            if s in ("q", "quit", "exit"):
                break
            print()
            try:
                target = float(s)
            except ValueError:
                continue
            f.move_to(min(100.0, max(0.0, target)) / 100)
    except (KeyboardInterrupt, EOFError):
        pass
    finally:
        stop.set()
        t.join(timeout=0.5)
        f.close()
        print()


if __name__ == "__main__":
    main()
