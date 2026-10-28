#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["smbus2>=0.4", "gpiozero>=2.0", "lgpio>=0.2.2"]
# ///
"""Live bar with the fader position + a prompt to send it to 0-100.

Usage: uv run machine.py
t flips the bar (depending on whether you view the fader from above or below).
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
    # Always writes on the line above the cursor without moving it: the prompt lives below.
    while not stop.is_set():
        sys.stdout.write("\0337\033[1A\r\033[2K" + bar_line(f.read()) + "\0338")
        sys.stdout.flush()
        time.sleep(1.0 / HZ)


def read_line(prompt: str) -> str:
    """input() that intercepts t to flip the bar."""
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
    print("Type 0-100 and Enter to move. t flips the bar. q to quit.")
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
