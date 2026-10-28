#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["smbus2>=0.4", "gpiozero>=2.0", "lgpio>=0.2.2"]
# ///
"""HTTP server exposing the fader. Everything over GET, JSON responses.

Usage: uv run controller.py [--host 0.0.0.0] [--port 8080]

  /state                          position + motor state
  /position                       position only, 0.0-1.0
  /move?to=0.3                    closed loop to the position (blocks, max TIMEOUT)
  /drive?dir=forward&speed=0.5&ms=200   raw push, stops by itself after ms
  /stop                           stops the motor
"""
import argparse
import json
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

from motor import Fader

MAX_DRIVE_MS = 1000

fader: Fader
# One motor command at a time; the second requester gets 409.
motor_lock = threading.Lock()
drive_timer: threading.Timer | None = None


def motor_state() -> dict:
    v = fader.motor.value
    direction = "stopped" if v == 0 else ("forward" if v > 0 else "backward")
    return {"direction": direction, "speed": abs(v), "busy": motor_lock.locked()}


def state() -> dict:
    return {"position": fader.read(), "motor": motor_state(), "t": time.time()}


def stop() -> dict:
    global drive_timer
    if drive_timer:
        drive_timer.cancel()
        drive_timer = None
    fader.motor.stop()
    return state()


def drive(direction: str, speed: float, ms: int) -> dict:
    global drive_timer
    ms = min(MAX_DRIVE_MS, max(1, ms))
    speed = min(1.0, max(0.0, speed))
    if drive_timer:
        drive_timer.cancel()
    (fader.motor.forward if direction == "forward" else fader.motor.backward)(speed)
    drive_timer = threading.Timer(ms / 1000, fader.motor.stop)
    drive_timer.daemon = True
    drive_timer.start()
    return state()


class Handler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        url = urlparse(self.path)
        q = {k: v[-1] for k, v in parse_qs(url.query).items()}
        try:
            code, body = self.route(url.path, q)
        except (KeyError, ValueError) as e:
            code, body = 400, {"error": f"bad param: {e}"}
        payload = json.dumps(body).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        self.wfile.write(payload)

    def route(self, path: str, q: dict) -> tuple[int, dict]:
        if path in ("/", "/state"):
            return 200, state()
        if path == "/position":
            return 200, {"position": fader.read()}
        if path == "/stop":
            with motor_lock:
                stop()
            return 200, state()
        if path == "/move":
            target = float(q["to"])
            if not motor_lock.acquire(blocking=False):
                return 409, {"error": "motor busy"}
            try:
                stop()
                reached = fader.move_to(target)
            finally:
                motor_lock.release()
            return 200, {"target": target, "reached": reached, **state()}
        if path == "/drive":
            direction = q.get("dir", "forward")
            if direction not in ("forward", "backward"):
                raise ValueError("dir must be forward|backward")
            if not motor_lock.acquire(blocking=False):
                return 409, {"error": "motor busy"}
            try:
                return 200, drive(direction, float(q.get("speed", 0.5)), int(q.get("ms", 200)))
            finally:
                motor_lock.release()
        return 404, {"error": "not found"}

    def log_message(self, fmt: str, *args) -> None:
        print(f"{self.address_string()} {fmt % args}")


def main() -> None:
    global fader
    p = argparse.ArgumentParser()
    p.add_argument("--host", default="0.0.0.0")
    p.add_argument("--port", type=int, default=8080)
    args = p.parse_args()
    fader = Fader()
    server = ThreadingHTTPServer((args.host, args.port), Handler)
    print(f"controller on http://{args.host}:{args.port}  (Ctrl+C to quit)")
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        stop()
        fader.close()
        server.server_close()


if __name__ == "__main__":
    main()
