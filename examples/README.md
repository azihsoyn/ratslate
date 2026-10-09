# Examples

Scripts that drive a board through `ratslate --api` — the same requests a
mouse click dispatches, so they're agents that happen to be shell loops.

## gh-actions-board.py — a GitHub Actions run, live

Jobs become boxes, `needs:` become connectors, and each box carries the
job's status as a badge: `○` pending, a spinner while it runs, `✓`/`✗`
when it's done, `⊘` skipped. Open the board in another pane and watch the
run flow through it; boxes an update touched flash as they change.

```sh
examples/gh-actions-board.py ci.canvas              # latest run of the current repo
examples/gh-actions-board.py ci.canvas --steps      # each job as a table of its steps
examples/gh-actions-board.py ci.canvas --run 123    # a specific run
ratslate ci.canvas                                  # in another pane; press `a` for flow
```

Needs `gh` (logged in) and `ratslate` on `PATH`. Standard library only;
PyYAML is used for the workflow file if installed, otherwise a small
built-in parser handles the usual `needs:` shapes.
