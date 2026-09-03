#!/usr/bin/env python3
"""Replay a cast recorded by record.py, with original timing."""
import base64, json, sys, time
sys.stdout.buffer.write(b"\x1b[2J\x1b[H")
sys.stdout.buffer.flush()
prev = 0.0
with open(sys.argv[1]) as f:
    for line in f:
        t, data = json.loads(line)
        dt = t - prev
        if dt > 0:
            time.sleep(min(dt, 2.0))
        prev = t
        sys.stdout.buffer.write(base64.b64decode(data))
        sys.stdout.buffer.flush()
