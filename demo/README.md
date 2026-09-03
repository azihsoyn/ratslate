# Regenerating demo.gif

The demo is a scripted session (keys and SGR mouse events) recorded
against a real `ratslate` in a fixed-size pty, then replayed inside
[vhs](https://github.com/charmbracelet/vhs):

```sh
# 1. prepare a board with the demo table (see events.txt for the story)
# 2. record: spawns the command in a 97x33 pty, feeds events.txt, captures output
python3 record.py demo.cast events.txt sh -c 'ratslate demo.canvas && printf "\n\$ ratslate demo.canvas --render\n\n" && ratslate demo.canvas --render && sleep 3'
# 3. render: vhs replays the cast and writes demo.gif
vhs demo.tape
```

The pty size (97x33) matches what vhs's own terminal comes out to at
the tape's font settings — measure with `stty size` inside a probe
tape if the settings change.
