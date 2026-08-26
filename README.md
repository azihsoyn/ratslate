# ratslate

An infinite canvas in the terminal. Boxes, arrows and text, placed and moved
with the mouse — and written out as ASCII, so the drawing can live in a README
or a comment rather than only on the screen it was made on.

Reads and writes [JSON Canvas](https://jsoncanvas.org) (`.canvas`), the format
Obsidian's Canvas uses, so a board can round-trip through Obsidian.

A `.md` file opens as a second mode: headings and lists become a
draggable mindmap, reordering and re-parenting written back to the
source as a minimal diff — a plain line move when nothing changes
depth, otherwise just the moved block's own heading level or list
indent.

## `--api`

Every mouse drag and keystroke is also a `Request` value, and there is
exactly one function that applies one: `dispatch`. `--api` is that
same function reached from the command line instead of a terminal —
not a second implementation of what a move or an edit means.

```sh
ratslate board.canvas --schema              # what a request/response looks like
ratslate board.canvas --api '{"type":"place","x":2,"y":2}'
ratslate plan.md --api '{"type":"move","id":6,"parent":2,"index":0}'
```

A JSON array runs as a batch, applied in order, replied to in order.
Nothing is written to disk until a `save` request says so — `--api` is
safe to use for a read-only `state` query.
