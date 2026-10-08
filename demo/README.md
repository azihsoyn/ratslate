# Regenerating demo.gif

The demo is a scripted session (keys and SGR mouse events) recorded
against a real `ratslate` in a fixed-size pty, then replayed inside
[vhs](https://github.com/charmbracelet/vhs):

```sh
# The board is imported from pipeline.mmd (a Mermaid flowchart, committed
# here), then edited with the mouse. RATSLATE_NO_IMAGES=1 skips the image
# capability probe — its terminal queries print as garbage in a dumb pty.
rm -f pipeline.canvas pipeline.canvas.crdt
# record: spawns the command in a 97x33 pty, feeds events.txt, captures output
python3 record.py demo.cast events.txt sh -c 'printf "$ ratslate pipeline.canvas --import pipeline.mmd\n"; RATSLATE_NO_IMAGES=1 ratslate pipeline.canvas --import pipeline.mmd; printf "\n$ ratslate pipeline.canvas --render\n\n"; ratslate pipeline.canvas --render; sleep 3'
# render: vhs replays the cast and writes ../demo.gif
vhs demo.tape
# tidy the files recording left behind (none of these are committed)
rm -f pipeline.canvas pipeline.canvas.crdt demo.cast
```

events.txt drives the mouse by screen coordinate, and the imported graph
lands at fixed positions (the layout is deterministic and the camera
opens at the origin), so the drag/colour targets are load-bearing — if
the layout changes, the coordinates need to follow.

The tape sleeps 1.2s (hidden) after launching so capture starts once
ratslate has taken the screen — otherwise frame 0 is the shell prompt
with the `play.py` command, which is what shows as the GIF thumbnail.

The pty size (97x33) matches what vhs's own terminal comes out to at
the tape's font settings — measure with `stty size` inside a probe
tape if the settings change.
