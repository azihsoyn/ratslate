# Examples

Sources for `ratslate --watch`: a script prints a sync document (items with
stable keys, links between them — `SyncDoc` in `ratslate --schema`) and
ratslate runs it on a timer, reconciling the board by key. The same document
is one `sync` request through `--api`, so nothing here is a second way in.

## gh-actions-board.py — a GitHub Actions run, live

Jobs become boxes, `needs:` become connectors, and each box carries the
job's status as a badge: `○` pending, a spinner while it runs, `✓`/`✗`
when it's done, `⊘` skipped. Watch it and see the run flow through the
board; boxes an update touched flash as they change, and keep wherever
you dragged them.

```sh
ratslate ci.canvas --watch examples/gh-actions-board.py --every 8             # latest run of the current repo
ratslate ci.canvas --watch 'examples/gh-actions-board.py --steps'             # each job as a table of its steps
ratslate ci.canvas --watch 'examples/gh-actions-board.py --run 123456789'     # a specific run
examples/gh-actions-board.py | head                                           # just the document
```

Press `a` in the TUI for flow pulses along the connectors. Needs `gh`
(logged in). Standard library only; PyYAML is used for the workflow file if
installed, otherwise a small built-in parser handles the usual `needs:`
shapes.
