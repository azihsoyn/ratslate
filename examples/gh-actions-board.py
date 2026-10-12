#!/usr/bin/env python3
"""A GitHub Actions run as a live ratslate board.

Prints the run as a sync document — jobs as items keyed by job name,
`needs:` as links, each job's status as its badge — and ratslate's
`--watch` does the rest: it runs this script on a timer, matches items
to boxes by key, and updates the board in place. Boxes keep wherever
you dragged them; only what changed flashes.

    ratslate ci.canvas --watch examples/gh-actions-board.py --every 8
    ratslate ci.canvas --watch 'examples/gh-actions-board.py --steps'       # each job as a table of its steps
    ratslate ci.canvas --watch 'examples/gh-actions-board.py --run 123456'  # a specific run

Each document is applied as one `sync` request — the same thing
`--api '{"type":"sync","doc":{…}}'` takes by hand. The format is
`SyncDoc` in `ratslate --schema`. Needs `gh` (logged in). Standard
library only.
"""
import argparse
import base64
import json
import re
import subprocess
import sys

GLYPH = {"pending": "○", "running": "⠋", "ok": "✓", "failed": "✗", "skipped": "⊘"}


def sh(*args):
    r = subprocess.run(list(args), capture_output=True, text=True)
    if r.returncode != 0:
        sys.exit(f"{' '.join(args[:3])}… failed:\n{r.stderr.strip()}")
    return r.stdout


def gh_json(*args):
    return json.loads(sh("gh", *args))


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


def item(job, with_steps):
    """A job's box: its name and running time, or with --steps a table
    headed by the name with one row per step and the step's own badge.
    Tables get an explicit size — they don't grow to fit on their own."""
    name = job["name"]
    it = {"key": f"job:{name}", "status": status_of(job)}
    if not with_steps:
        d = duration(job)
        it["text"] = f"{name}\n{d}" if d else name
        return it
    steps = job.get("steps", [])
    rows = [f"| {name} | |", "| --- | --- |"]
    for s in steps:
        rows.append(f"| {s['name'][:30]} | {GLYPH[status_of(s)]} |")
    widest = max([len(name)] + [len(s["name"][:30]) for s in steps])
    it.update(text="\n".join(rows), width=min(widest + 10, 48), height=len(steps) + 4)
    return it


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--repo", help="owner/name (default: the current repo)")
    ap.add_argument("--run", help="run id (default: the latest run)")
    ap.add_argument("--workflow", help="only consider runs of this workflow file/name when picking the latest")
    ap.add_argument("--steps", action="store_true", help="draw each job as a table of its steps")
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

    jobs = gh_json("run", "view", run_id, "-R", repo, "--json", "jobs")["jobs"]

    # Connectors from `needs`, matrix instances included on both ends.
    by_yaml = {}
    for j in jobs:
        yid = job_yaml_id(j["name"], needs)
        if yid:
            by_yaml.setdefault(yid, []).append(j["name"])
    links = []
    for j in jobs:
        yid = job_yaml_id(j["name"], needs)
        for dep in needs.get(yid, []) if yid else []:
            for src in by_yaml.get(dep, []):
                links.append({"from": f"job:{src}", "to": f"job:{j['name']}"})

    # prune: a new run's jobs replace the old run's, instead of piling up.
    print(json.dumps({"items": [item(j, args.steps) for j in jobs], "links": links, "prune": True}, indent=1))


if __name__ == "__main__":
    main()
