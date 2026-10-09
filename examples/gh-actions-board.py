#!/usr/bin/env python3
"""A GitHub Actions run as a live ratslate board.

Jobs become boxes, `needs:` become connectors, and each box carries the
job's status as a badge: pending, a spinner while it runs, a check or a
cross when it's done. Open the board in ratslate in another pane and
watch the run flow through it.

    examples/gh-actions-board.py ci.canvas                    # latest run of this repo
    examples/gh-actions-board.py ci.canvas --run 123456789    # a specific run
    examples/gh-actions-board.py ci.canvas --steps            # each job as a table of its steps
    ratslate ci.canvas                                        # in another pane

Everything goes through `ratslate --api`, the same requests a mouse
click dispatches, and reaches the open TUI live through the CRDT
sidecar — this script is just an agent that happens to be a shell
loop. Needs `gh` (logged in) and `ratslate` on PATH. Standard library
only.
"""
import argparse
import base64
import json
import os
import re
import subprocess
import sys
import time

GLYPH = {"pending": "○", "running": "⠋", "ok": "✓", "failed": "✗", "skipped": "⊘"}


def sh(*args):
    r = subprocess.run(list(args), capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"{' '.join(args[:3])}… failed:\n{r.stderr.strip()}")
    return r.stdout


def gh_json(*args):
    return json.loads(sh("gh", *args))


def api(canvas, reqs):
    """One `--api` batch; returns the list of results."""
    out = sh("ratslate", canvas, "--api", json.dumps(reqs))
    res = json.loads(out)["result"]
    return res if isinstance(res, list) else [res]


def status_of(job):
    """GitHub's status/conclusion pair → a ratslate badge."""
    st, con = job.get("status"), job.get("conclusion")
    if st in ("queued", "waiting", "pending", "requested"):
        return "pending"
    if st == "in_progress":
        return "running"
    if con == "success":
        return "ok"
    if con in ("failure", "timed_out", "action_required", "startup_failure"):
        return "failed"
    if con in ("skipped", "cancelled", "neutral"):
        return "skipped"
    return "pending"


def duration(job):
    a, b = job.get("startedAt"), job.get("completedAt")
    if not a:
        return ""
    import datetime as dt
    fmt = "%Y-%m-%dT%H:%M:%SZ"
    start = dt.datetime.strptime(a, fmt)
    end = dt.datetime.strptime(b, fmt) if b else dt.datetime.utcnow()
    s = int((end - start).total_seconds())
    return f"{s // 60}m{s % 60:02d}s" if s >= 60 else f"{s}s"


def needs_from_yaml(text):
    """{job_id: [needs...]} from a workflow file. Uses PyYAML if present;
    otherwise a small indentation walk that understands the common
    shapes (`needs: x`, `needs: [a, b]`, and a `- a` list)."""
    try:
        import yaml  # type: ignore
        doc = yaml.safe_load(text)
        jobs = doc.get("jobs", {}) or {}
        out = {}
        for jid, spec in jobs.items():
            n = (spec or {}).get("needs", [])
            out[jid] = [n] if isinstance(n, str) else list(n or [])
        return out
    except ImportError:
        pass
    out, job, in_jobs = {}, None, False
    lines = text.splitlines()
    for i, line in enumerate(lines):
        if re.match(r"^jobs:\s*$", line):
            in_jobs = True
            continue
        if not in_jobs:
            continue
        if re.match(r"^\S", line):  # left the jobs block
            break
        m = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if m:
            job = m.group(1)
            out[job] = []
            continue
        if job is None:
            continue
        m = re.match(r"^    needs:\s*(.*)$", line)
        if m:
            rest = m.group(1).strip()
            if rest.startswith("["):
                out[job] = [x.strip().strip("'\"") for x in rest.strip("[]").split(",") if x.strip()]
            elif rest:
                out[job] = [rest.strip("'\"")]
            else:
                j = i + 1
                while j < len(lines) and re.match(r"^      - ", lines[j]):
                    out[job].append(lines[j].split("-", 1)[1].strip().strip("'\""))
                    j += 1
    return out


def transitive_reduction(needs):
    """Drop a dependency that's already implied by a longer path — a
    workflow that lists `plan` under every job draws as a pipeline, not
    a web of arrows all pointing back at the first box."""
    def reaches(a, b, seen):
        for n in needs.get(a, []):
            if n == b or (n not in seen and (seen.add(n) or reaches(n, b, seen))):
                return True
        return False
    out = {}
    for job, deps in needs.items():
        out[job] = [d for d in deps if not any(o != d and reaches(o, d, set()) for o in deps)]
    return out


def job_yaml_id(job_name, yaml_ids):
    """A run's job name is the job id, or `id (matrix values)`."""
    if job_name in yaml_ids:
        return job_name
    base = re.sub(r"\s*\(.*\)\s*$", "", job_name)
    return base if base in yaml_ids else None


def box_text(job, with_steps):
    """A job's box: its name and running time, or with --steps a table
    headed by the name with one row per step and the step's own badge."""
    name = job["name"]
    if not with_steps:
        d = duration(job)
        return f"{name}\n{d}" if d else name
    rows = [f"| {name} | |", "| --- | --- |"]
    for s in job.get("steps", []):
        rows.append(f"| {s['name'][:30]} | {GLYPH[status_of(s)]} |")
    return "\n".join(rows)


def box_size(job, with_steps):
    """Wide enough for the longest line, tall enough for every row —
    tables don't auto-grow from the API, only from the keyboard."""
    if not with_steps:
        return 30, 4
    steps = job.get("steps", [])
    widest = max([len(job["name"])] + [len(s["name"][:30]) for s in steps])
    return min(widest + 10, 48), len(steps) + 4


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("canvas", help="the .canvas file to draw into (created if missing)")
    ap.add_argument("--repo", help="owner/name (default: the current repo)")
    ap.add_argument("--run", help="run id (default: the latest run)")
    ap.add_argument("--workflow", help="only consider runs of this workflow file/name when picking the latest")
    ap.add_argument("--interval", type=float, default=8, help="seconds between polls (default 8)")
    ap.add_argument("--steps", action="store_true", help="draw each job as a table of its steps")
    ap.add_argument("--once", action="store_true", help="draw the current state and exit")
    ap.add_argument("--all-edges", action="store_true", help="draw every `needs:` edge, even ones implied by a longer path")
    args = ap.parse_args()

    repo = args.repo or gh_json("repo", "view", "--json", "nameWithOwner")["nameWithOwner"]
    run_id = args.run
    if not run_id:
        q = ["run", "list", "-R", repo, "--limit", "1", "--json", "databaseId"]
        if args.workflow:
            q += ["--workflow", args.workflow]
        runs = gh_json(*q)
        if not runs:
            sys.exit("no runs found")
        run_id = str(runs[0]["databaseId"])

    meta = gh_json("api", f"repos/{repo}/actions/runs/{run_id}")
    wf_text = base64.b64decode(gh_json("api", f"repos/{repo}/contents/{meta['path']}?ref={meta['head_sha']}")["content"]).decode()
    needs = needs_from_yaml(wf_text)
    if not args.all_edges:
        needs = transitive_reduction(needs)

    # Which box is which job, remembered beside the canvas so a re-run
    # of this script updates the same boxes instead of drawing new ones.
    side = args.canvas + ".gh.json"
    book = json.load(open(side)) if os.path.exists(side) else {}
    if book.get("run") != run_id:
        book = {"run": run_id, "boxes": {}}

    last = {}
    while True:
        run = gh_json("run", "view", run_id, "-R", repo, "--json", "jobs,status,conclusion,workflowName,displayTitle")
        jobs = run["jobs"]

        # First sight of a job: a box for it.
        new = [j for j in jobs if j["name"] not in book["boxes"]]
        if new:
            res = api(args.canvas, [{"type": "place", "x": 0, "y": 0, "w": box_size(j, args.steps)[0], "h": box_size(j, args.steps)[1]} for j in new])
            for j, r in zip(new, res):
                book["boxes"][j["name"]] = r["result"]["id"]
            # Connectors from `needs`, matrix instances included on both ends.
            by_yaml = {}
            for j in jobs:
                yid = job_yaml_id(j["name"], needs)
                if yid:
                    by_yaml.setdefault(yid, []).append(j["name"])
            edges = []
            done = set(book.get("edges", []))
            for j in new:
                yid = job_yaml_id(j["name"], needs)
                for dep in needs.get(yid, []) if yid else []:
                    for src in by_yaml.get(dep, []):
                        key = f"{src}->{j['name']}"
                        if key not in done:
                            edges.append({"type": "connect", "from": book["boxes"][src], "to": book["boxes"][j["name"]]})
                            done.add(key)
            book["edges"] = sorted(done)
            reqs = [{"type": "set_text", "id": book["boxes"][j["name"]], "text": box_text(j, args.steps)} for j in new] + edges
            reqs.append({"type": "layout"})
            api(args.canvas, reqs)

        # Status and text, only where something changed.
        reqs = []
        for j in jobs:
            st = status_of(j)
            text = box_text(j, args.steps)
            key = (st, text)
            if last.get(j["name"]) != key:
                bid = book["boxes"][j["name"]]
                reqs += [{"type": "set_status", "id": bid, "status": st}, {"type": "set_text", "id": bid, "text": text}]
                last[j["name"]] = key
        if reqs:
            api(args.canvas, reqs + [{"type": "save"}])
        json.dump(book, open(side, "w"))

        done = run["status"] == "completed"
        line = f"{run['workflowName']} · {run.get('displayTitle', '')[:40]} · {run['status']}" + (f" · {run['conclusion']}" if done else "")
        print(line + "   " + " ".join(f"{GLYPH[status_of(j)]}" for j in jobs), flush=True)
        if done or args.once:
            break
        time.sleep(args.interval)


if __name__ == "__main__":
    main()
