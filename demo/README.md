# Regenerating demo.gif

The demo is a scripted session (keys and SGR mouse events) recorded
against a real `ratslate` in a fixed-size pty, then replayed inside
[vhs](https://github.com/charmbracelet/vhs):

```sh
# 1. demo.canvas (the starting board — just the table) is committed here.
#    Recording QUITS ratslate, which auto-saves it with the demo's edits
#    (Alice 30->31, Tokyo->Kyoto), so restore it to pristine first:
git checkout demo.canvas
rm -f demo.canvas.crdt
# 2. record: spawns the command in a 97x33 pty, feeds events.txt, captures output
python3 record.py demo.cast events.txt sh -c 'ratslate demo.canvas && printf "\n\$ ratslate demo.canvas --render\n\n" && ratslate demo.canvas --render && sleep 3'
# 3. render: vhs replays the cast and writes demo.gif
vhs demo.tape
# 4. restore the mutated canvas again before committing
git checkout demo.canvas && rm -f demo.canvas.crdt
```

The table's position in demo.canvas is load-bearing: events.txt
double-clicks Alice's Age cell by screen coordinate, so moving the
table breaks the cell-edit part of the demo.

The tape sleeps 1.2s (hidden) after launching so capture starts once
ratslate has taken the screen — otherwise frame 0 is the shell prompt
with the `play.py` command, which is what shows as the GIF thumbnail.

The pty size (97x33) matches what vhs's own terminal comes out to at
the tape's font settings — measure with `stty size` inside a probe
tape if the settings change.
