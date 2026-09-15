#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["smbus2>=0.4", "gpiozero>=2.0", "lgpio>=0.2.2"]
# ///
"""Barra en vivo con la posicion del fader + prompt para mandarlo a 0-100.

Uso: uv run machine.py
t invierte la barra (segun mires el fader desde arriba o desde abajo).
"""
import sys
import termios
import threading
import time
import tty

from motor import Fader

WIDTH = 40
HZ = 20
flipped = False


def bar_line(pos: float) -> str:
    filled = int(pos * WIDTH)
    fill, rest = "#" * filled, "-" * (WIDTH - filled)
    bar = rest + fill if flipped else fill + rest
    return f"[{bar}] {pos * 100:5.1f}%"


def display_loop(f: Fader, stop: threading.Event) -> None:
    # Escribe siempre en la linea encima del cursor sin moverlo: el prompt vive debajo.
    while not stop.is_set():
        sys.stdout.write("\0337\033[1A\r\033[2K" + bar_line(f.read()) + "\0338")
        sys.stdout.flush()
        time.sleep(1.0 / HZ)


def read_line(prompt: str) -> str:
    """input() que intercepta t para invertir la barra."""
    global flipped
    sys.stdout.write(prompt)
    sys.stdout.flush()
    buf = ""
    while True:
        k = sys.stdin.read(1)
        if k == "t":
            flipped = not flipped
        elif k in ("\r", "\n"):
            sys.stdout.write("\n")
            sys.stdout.flush()
            return buf
        elif k in ("\x7f", "\x08"):
            if buf:
                buf = buf[:-1]
                sys.stdout.write("\b \b")
                sys.stdout.flush()
        elif k in ("\x03", "\x04"):
            raise KeyboardInterrupt
        elif k and k.isprintable():
            buf += k
            sys.stdout.write(k)
            sys.stdout.flush()


def main() -> None:
    f = Fader()
    stop = threading.Event()
    t = threading.Thread(target=display_loop, args=(f, stop), daemon=True)
    print("Escribe 0-100 y Enter para mover. t invierte la barra. q para salir.")
    print()
    fd = sys.stdin.fileno()
    old = termios.tcgetattr(fd)
    tty.setcbreak(fd)
    t.start()
    try:
        while True:
            s = read_line("> ").strip().lower()
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
        termios.tcsetattr(fd, termios.TCSADRAIN, old)
        stop.set()
        t.join(timeout=0.5)
        f.close()
        print()


if __name__ == "__main__":
    main()
