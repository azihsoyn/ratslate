#!/usr/bin/env python3
"""Spawn a command in a fixed-size pty, feed it a scripted event stream,
record everything it writes (with timestamps) into a JSONL cast file."""
import base64
import fcntl
import json
import os
import pty
import struct
import sys
import termios
import threading
import time

COLS, ROWS = 97, 33

def main():
    cast_path = sys.argv[1]
    script_path = sys.argv[2]
    cmd = sys.argv[3:]
    events = []  # (delay_seconds, bytes) parsed from script file
    with open(script_path) as f:
        for line in f:
            line = line.rstrip("\n")
            if not line or line.startswith("#"):
                continue
            delay, payload = line.split(" ", 1)
            events.append((float(delay), payload.encode().decode("unicode_escape").encode("latin1")))

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.execvp(cmd[0], cmd)
    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", ROWS, COLS, 0, 0))

    out = open(cast_path, "w")
    start = time.monotonic()
    done = threading.Event()

    def reader():
        while True:
            try:
                data = os.read(fd, 65536)
            except OSError:
                break
            if not data:
                break
            t = time.monotonic() - start
            out.write(json.dumps([round(t, 4), base64.b64encode(data).decode()]) + "\n")
        done.set()

    threading.Thread(target=reader, daemon=True).start()
    time.sleep(0.5)
    for delay, payload in events:
        time.sleep(delay)
        os.write(fd, payload)
    done.wait(timeout=10)
    out.close()
    os.close(fd)

main()
