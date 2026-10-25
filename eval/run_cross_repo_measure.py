#!/usr/bin/env python3
"""Measure the multi-repo registry, ``cross_repo_search_tool``, and the watch daemon.

Every registry and daemon command runs with ``HOME`` pointed at
``<work-dir>/home`` so the real ``~/.dagayn`` is never read or written; the
script checks the real files' mtimes before and after.  ``<work-dir>`` holds
mutable clones of ``cfwdon`` and ``tstodon`` (``git clone --no-local``), which
the script resets and rebuilds on every run.

Phases, each recorded as pass/fail steps in ``summary.json``:

1. registry: register, ``repos``, ``list_repos_tool``, unregister round trip,
   and a registered repo without a graph (must be reported, not dropped).
2. usefulness: each query in ``QUERIES`` through ``cross_repo_search_tool``
   (one call) and ``semantic_search_nodes_tool`` per repo (one call each);
   compares the per-repo top-``LIMIT`` qualified-name sets, reply sizes, and
   how the merged list interleaves repositories.
3. daemon: ``daemon add`` both, ``daemon start``, append a new function to a
   file in each clone ``EDITS_PER_REPO`` times, time until the node row exists
   and until ``cross_repo_search_tool`` returns it, then ``daemon stop`` and
   look for stray watcher processes.

``cross_repo_judgments.yaml`` (next to this file) holds hand verdicts on the
top merged hits; the script refuses a judged id that is not a node in that
repo's graph.

    python eval/run_cross_repo_measure.py --work-dir DIR --out DIR
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sqlite3
import subprocess
import time
from collections import Counter
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent
VENV_BIN = HERE.parent / ".venv" / "bin"
DAGAYN = VENV_BIN / "dagayn"
JUDGMENTS = HERE / "cross_repo_judgments.yaml"
REPOS = ("cfwdon", "tstodon")
LIMIT = 10
EDITS_PER_REPO = 3
LATENCY_TIMEOUT = 60.0

# (id, query, expectation): "concept" = shared idea, "both" = exact name in
# both repos, "cfwdon"/"tstodon" = exact name in that repo only.
QUERIES = [
    ("follow-request", "follow request", "concept"),
    ("inbox-handler", "inbox activity handler", "concept"),
    ("webfinger", "WebFinger", "concept"),
    ("http-sig-verify", "HTTP signature verification", "concept"),
    ("outbox-retry", "outbox delivery retry", "concept"),
    ("hashtag-extract", "hashtag extraction", "concept"),
    ("nodeinfo", "nodeinfo document", "concept"),
    ("actor-document", "actor document", "concept"),
    ("parseSignatureHeader", "parseSignatureHeader", "both"),
    ("buildSigningString", "buildSigningString", "both"),
    ("authorizeFollowRequest", "authorizeFollowRequest", "both"),
    ("sha256DigestHeader", "sha256DigestHeader", "both"),
    ("verify_http_signature_bytes", "verify_http_signature_bytes", "cfwdon"),
    ("build_nodeinfo_21_document", "build_nodeinfo_21_document", "cfwdon"),
    ("D1Database", "D1Database", "cfwdon"),
    ("fetchWebfingerActorUri", "fetchWebfingerActorUri", "tstodon"),
]

# Files the daemon phase appends probe functions to.
EDIT_TARGETS = {
    "tstodon": ("packages/domain/src/follow-request.ts", "ts"),
    "cfwdon": ("crates/cfwdon-models/src/local_follow_request.rs", "rs"),
}
REAL_HOME_FILES = ("registry.json", "watch.toml", "daemon.pid", "daemon-state.json")


class Run:
    def __init__(self, work: Path, out: Path) -> None:
        self.work = work
        self.out = out
        self.home = work / "home"
        self.env = dict(os.environ, HOME=str(self.home), PATH=f"{VENV_BIN}:{os.environ['PATH']}")
        self.steps: list[dict] = []
        self.errors: list[str] = []
        self.raw_n = 0

    def cmd(self, argv: list[str], timeout: float = 300) -> tuple[int, str, str, float]:
        start = time.monotonic()
        try:
            proc = subprocess.run(
                argv, capture_output=True, text=True, timeout=timeout, env=self.env, cwd=self.work
            )
        except subprocess.TimeoutExpired:
            return -1, "", "timeout", time.monotonic() - start
        return proc.returncode, proc.stdout, proc.stderr, time.monotonic() - start

    def dagayn(self, *args: str, timeout: float = 300) -> tuple[int, str, str, float]:
        return self.cmd([str(DAGAYN), *args], timeout=timeout)

    def tool(self, name: str, repo: str | None = None, save: str | None = None, **kw) -> tuple:
        argv = ["tool", name]
        if repo:
            argv += ["--repo", str(self.work / repo)]
        for key, value in kw.items():
            argv += ["--arg", f"{key}={json.dumps(value)}"]
        rc, out, err, secs = self.dagayn(*argv)
        if save:
            (self.out / f"{save}.json").write_text(out)
        try:
            reply = json.loads(out)
        except json.JSONDecodeError:
            self.errors.append(f"{name} {kw}: unparsable rc={rc} {err[-200:]}")
            return out, None, secs
        if reply.get("status") != "ok":
            self.errors.append(f"{name} {kw}: status={reply.get('status')}")
        return out, reply, secs

    def step(self, name: str, ok: bool, detail: object = None, secs: float | None = None) -> bool:
        row = {"step": name, "ok": bool(ok), "detail": detail}
        if secs is not None:
            row["secs"] = round(secs, 2)
        self.steps.append(row)
        print(("PASS " if ok else "FAIL ") + name, "" if detail is None else str(detail)[:200])
        return ok


def git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args], capture_output=True, text=True, check=True
    ).stdout


def node_exists(db: Path, qualified_name: str) -> bool:
    conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True)
    try:
        return (
            conn.execute(
                "SELECT 1 FROM nodes WHERE qualified_name = ? LIMIT 1", (qualified_name,)
            ).fetchone()
            is not None
        )
    finally:
        conn.close()


def node_named(db: Path, name: str) -> bool:
    try:
        conn = sqlite3.connect(f"file:{db}?mode=ro", uri=True, timeout=1)
        try:
            return (
                conn.execute("SELECT 1 FROM nodes WHERE name = ?", (name,)).fetchone() is not None
            )
        finally:
            conn.close()
    except sqlite3.Error:
        return False


def real_home_mtimes() -> dict[str, float | None]:
    base = Path(os.path.expanduser("~")) / ".dagayn"
    return {f: (base / f).stat().st_mtime if (base / f).exists() else None for f in REAL_HOME_FILES}


def stray_pids(work: Path) -> list[str]:
    proc = subprocess.run(["pgrep", "-fl", str(work)], capture_output=True, text=True)
    # Only watcher children: the shell that launched this script also names
    # the work dir on its command line.
    shells = ("sh", "bash", "zsh")
    return [
        line
        for line in proc.stdout.splitlines()
        if "dagayn watch --repo" in line
        and len(line.split()) > 1
        and Path(line.split()[1]).name not in shells
    ]


def setup(run: Run) -> None:
    assert run.home != Path(os.path.expanduser("~")), "HOME override must differ from the real home"
    shutil.rmtree(run.home / ".dagayn", ignore_errors=True)
    run.home.mkdir(parents=True, exist_ok=True)
    for repo in REPOS:
        path = run.work / repo
        git(path, "checkout", "-q", "--", ".")
        git(path, "clean", "-qfd", "-e", ".dagayn")
        rc, out, err, secs = run.dagayn("build", "--repo", str(path), timeout=900)
        run.step(f"build {repo}", rc == 0, out.strip().splitlines()[:1], secs)


def registry_phase(run: Run) -> None:
    rc, out, _, _ = run.dagayn("repos")
    run.step("repos empty before register", rc == 0 and "No repositories" in out, out.strip())
    for repo in REPOS:
        rc, out, err, secs = run.dagayn("register", str(run.work / repo), "--alias", repo)
        run.step(f"register {repo}", rc == 0 and "Registered" in out, (out + err).strip(), secs)
    rc, out, _, _ = run.dagayn("register", str(run.work / "cfwdon"), "--alias", "cfwdon")
    entries = json.loads((run.home / ".dagayn" / "registry.json").read_text())["repos"]
    run.step("register is idempotent", len(entries) == 2, entries)
    rc, out, _, _ = run.dagayn("repos")
    run.step("repos lists both", rc == 0 and all(r in out for r in REPOS), out.strip())
    text, reply, secs = run.tool("list_repos_tool", save="list_repos")
    aliases = sorted(r.get("alias", "") for r in (reply or {}).get("repos", []))
    run.step(
        "list_repos_tool", aliases == sorted(REPOS), {"aliases": aliases, "chars": len(text)}, secs
    )

    # A registered checkout with no graph must show up as skipped, not vanish.
    bare = run.work / "nograph"
    shutil.rmtree(bare, ignore_errors=True)
    bare.mkdir()
    subprocess.run(["git", "init", "-q", str(bare)], check=True)
    run.dagayn("register", str(bare), "--alias", "nograph")
    _, reply, _ = run.tool("cross_repo_search_tool", query="WebFinger", limit=3)
    skipped = (reply or {}).get("repos_skipped", [])
    codes = [m.get("reason_code") for m in (reply or {}).get("missingness", [])]
    run.step(
        "unbuilt repo reported as skipped",
        skipped == [{"repo": "nograph", "reason": "no_graph"}]
        and "registered_repos_not_searched" in codes,
        {"skipped": skipped, "missingness": codes},
    )
    rc, out, _, _ = run.dagayn("unregister", "nograph")
    entries = json.loads((run.home / ".dagayn" / "registry.json").read_text())["repos"]
    run.step("unregister by alias", rc == 0 and len(entries) == 2, out.strip())
    shutil.rmtree(bare, ignore_errors=True)


def interleave(results: list[dict]) -> dict:
    labels = [r.get("repo") for r in results]
    switches = sum(1 for a, b in zip(labels, labels[1:]) if a != b)
    scores = [r.get("score", 0) for r in results]
    first = {repo: labels.index(repo) if repo in labels else None for repo in REPOS}
    return {
        "top10_by_repo": dict(Counter(labels[:LIMIT])),
        "first_position": first,
        "switches": switches,
        "sorted_by_score": scores == sorted(scores, reverse=True),
        "all_hits_carry_repo": all(r.get("repo") and r.get("repo_path") for r in results),
    }


def usefulness_phase(run: Run) -> list[dict]:
    rows = []
    for qid, query, expect in QUERIES:
        ctext, creply, csecs = run.tool(
            "cross_repo_search_tool", save=f"cross_{qid}", query=query, limit=LIMIT
        )
        cresults = (creply or {}).get("results", [])
        row: dict = {"id": qid, "query": query, "expect": expect, "cross_chars": len(ctext)}
        row["cross_secs"] = round(csecs, 2)
        row["cross_results"] = len(cresults)
        row["cross_has_next"] = bool((creply or {}).get("next"))
        row["interleave"] = interleave(cresults)
        per_chars = 0
        equal = True
        row["per_repo"] = {}
        min_chars = 0
        for repo in REPOS:
            stext, sreply, _ = run.tool(
                "semantic_search_nodes_tool",
                repo=repo,
                save=f"sem_{qid}_{repo}",
                query=query,
                limit=LIMIT,
            )
            per_chars += len(stext)
            mtext, _, _ = run.tool(
                "semantic_search_nodes_tool",
                repo=repo,
                save=f"sem_min_{qid}_{repo}",
                query=query,
                limit=LIMIT,
                detail_level="minimal",
            )
            min_chars += len(mtext)
            sem = [r["qualified_name"] for r in (sreply or {}).get("results", [])][:LIMIT]
            crs = [r["qualified_name"] for r in cresults if r.get("repo") == repo][:LIMIT]
            same_set = set(sem) == set(crs)
            equal = equal and same_set
            row["per_repo"][repo] = {
                "sem_n": len(sem),
                "cross_n": len(crs),
                "same_set": same_set,
                "same_order": sem == crs,
                "only_sem": sorted(set(sem) - set(crs))[:5],
                "only_cross": sorted(set(crs) - set(sem))[:5],
                "sem_top1": sem[0] if sem else None,
                "sem_exact_match_count": ((sreply or {}).get("exactness") or {}).get(
                    "exact_match_count"
                ),
                "sem_zero_result_reason": (sreply or {}).get("zero_result_reason"),
            }
        # Bytes the merged reply spends on provenance: a full repo_path on
        # every hit plus the per-repo embedding_health blocks.
        row["cross_provenance_chars"] = sum(
            len(json.dumps({"repo_path": r.get("repo_path")})) for r in cresults
        ) + len(json.dumps((creply or {}).get("repo_search_modes", {})))
        row["per_repo_chars"] = per_chars
        row["per_repo_minimal_chars"] = min_chars
        row["hits_equal"] = equal
        rows.append(row)
        print(
            qid,
            "equal" if equal else "DIFF",
            len(ctext),
            per_chars,
            row["interleave"]["top10_by_repo"],
        )
    return rows


def wait_for_log(path: Path, needle: str, timeout: float) -> float | None:
    start = time.monotonic()
    while time.monotonic() - start < timeout:
        if path.exists() and needle in path.read_text(errors="replace"):
            return time.monotonic() - start
        time.sleep(0.2)
    return None


def probe_source(name: str, lang: str) -> str:
    if lang == "ts":
        return f"\nexport function {name}(value: number): number {{\n  return value + 1;\n}}\n"
    return f"\npub fn {name}(value: u32) -> u32 {{\n    value + 1\n}}\n"


def daemon_phase(run: Run) -> list[dict]:
    before = stray_pids(run.work)
    run.step("no watcher processes before start", not before, before)
    for repo in REPOS:
        rc, out, err, _ = run.dagayn("daemon", "add", str(run.work / repo), "--alias", repo)
        run.step(f"daemon add {repo}", rc == 0, (out + err).strip())
    rc, out, err, secs = run.dagayn("daemon", "start", timeout=60)
    run.step("daemon start", rc == 0, (out + err).strip(), secs)
    logs = run.home / ".dagayn" / "logs"
    for repo in REPOS:
        ready = wait_for_log(logs / f"{repo}.log", "Watching", 60)
        run.step(f"watcher {repo} ready", ready is not None, {"secs_after_start": ready})
    rc, out, err, _ = run.dagayn("daemon", "status")
    run.step("daemon status shows running", rc == 0 and "running" in out.lower(), out.strip())

    samples = []
    for i in range(EDITS_PER_REPO):
        for repo in REPOS:
            rel, lang = EDIT_TARGETS[repo]
            new_file = i == EDITS_PER_REPO - 1
            if new_file:
                rel = str(Path(rel).parent / f"cross_repo_probe.{lang}")
            name = f"crossRepoProbe{lang.upper()}{i}x{int(time.time())}"
            if lang == "rs":
                name = f"cross_repo_probe_rs_{i}_{int(time.time())}"
            db = run.work / repo / ".dagayn" / "graph.db"
            t0 = time.monotonic()
            with open(run.work / repo / rel, "w" if new_file else "a", encoding="utf-8") as fh:
                fh.write(probe_source(name, lang))
            node_at = None
            search_at = None
            polls = 0
            while time.monotonic() - t0 < LATENCY_TIMEOUT:
                if node_at is None and node_named(db, name):
                    node_at = time.monotonic() - t0
                if node_at is not None:
                    polls += 1
                    _, reply, _ = run.tool("cross_repo_search_tool", query=name, limit=3)
                    hits = [
                        r
                        for r in (reply or {}).get("results", [])
                        if r.get("name") == name and r.get("repo") == repo
                    ]
                    if hits:
                        search_at = time.monotonic() - t0
                        break
                time.sleep(0.1)
            sample = {
                "repo": repo,
                "name": name,
                "edit": "new file" if new_file else "append to existing file",
                "node_row_secs": None if node_at is None else round(node_at, 2),
                "search_visible_secs": None if search_at is None else round(search_at, 2),
                "search_polls": polls,
            }
            samples.append(sample)
            run.step(f"daemon picked up {name}", search_at is not None, sample)
            time.sleep(1.0)

    pid_path = run.home / ".dagayn" / "daemon.pid"
    daemon_pid = pid_path.read_text().strip() if pid_path.exists() else None
    live = stray_pids(run.work)
    run.step("watchers running before stop", len(live) >= len(REPOS), live)
    rc, out, err, secs = run.dagayn("daemon", "stop", timeout=60)
    run.step("daemon stop", rc == 0, (out + err).strip(), secs)
    time.sleep(1.0)
    left = stray_pids(run.work)
    alive = daemon_pid is not None and subprocess.run(["kill", "-0", daemon_pid]).returncode == 0
    run.step(
        "no stray processes after stop",
        not left and not alive,
        {"left": left, "daemon_alive": alive},
    )
    run.step("pid file removed", not pid_path.exists())
    for line in left:
        subprocess.run(["kill", line.split()[0]])

    # Control: with the daemon stopped, nothing else (tool calls included)
    # should index an edit, or the daemon is not what made it visible.
    rel, lang = EDIT_TARGETS["tstodon"]
    name = f"crossRepoControl{int(time.time())}"
    with open(run.work / "tstodon" / rel, "a", encoding="utf-8") as fh:
        fh.write(probe_source(name, lang))
    time.sleep(5)
    db = run.work / "tstodon" / ".dagayn" / "graph.db"
    _, reply, _ = run.tool("cross_repo_search_tool", query=name, limit=3)
    seen = any(r.get("name") == name for r in (reply or {}).get("results", []))
    run.step(
        "control: no update without daemon",
        not node_named(db, name) and not seen,
        {"node_row": node_named(db, name), "search_hit": seen},
    )
    return samples


def judgments_phase(run: Run, rows: list[dict]) -> dict:
    if not JUDGMENTS.exists():
        return {}
    data = yaml.safe_load(JUDGMENTS.read_text())
    query_ids = {r["id"] for r in rows}
    counts: Counter = Counter()
    for item in data["judgments"]:
        if item["query"] not in query_ids:
            raise SystemExit(f"judged query {item['query']} is not in QUERIES")
        db = run.work / item["repo"] / ".dagayn" / "graph.db"
        if item.get("id") and not node_exists(db, item["id"]):
            raise SystemExit(f"judged id {item['id']} is not a node in {item['repo']}")
        counts[f"{item['kind']}:{item['verdict']}"] += 1
    return dict(counts)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--skip-daemon", action="store_true")
    args = parser.parse_args()
    args.out.mkdir(parents=True, exist_ok=True)
    run = Run(args.work_dir.resolve(), args.out)
    real_before = real_home_mtimes()

    setup(run)
    registry_phase(run)
    rows = usefulness_phase(run)
    samples = [] if args.skip_daemon else daemon_phase(run)
    judged = judgments_phase(run, rows)
    real_after = real_home_mtimes()
    run.step("real ~/.dagayn untouched", real_before == real_after, real_after)

    n = len(rows)
    lat = [s["search_visible_secs"] for s in samples if s["search_visible_secs"] is not None]
    summary = {
        "queries": n,
        "hits_equal": sum(r["hits_equal"] for r in rows),
        "per_repo_same_order": sum(p["same_order"] for r in rows for p in r["per_repo"].values()),
        "cross_chars_total": sum(r["cross_chars"] for r in rows),
        "per_repo_chars_total": sum(r["per_repo_chars"] for r in rows),
        "per_repo_minimal_chars_total": sum(r["per_repo_minimal_chars"] for r in rows),
        "cross_provenance_chars_total": sum(r["cross_provenance_chars"] for r in rows),
        "calls": {"cross": n, "per_repo": n * len(REPOS)},
        "both_repos_in_top10": sum(len(r["interleave"]["top10_by_repo"]) == 2 for r in rows),
        "cross_reply_has_next": sum(r["cross_has_next"] for r in rows),
        "latency_samples": samples,
        "latency_search_visible": sorted(lat),
        "judgments": judged,
        "steps_failed": [s["step"] for s in run.steps if not s["ok"]],
        "errors": run.errors,
    }
    (args.out / "rows.json").write_text(json.dumps(rows, indent=2))
    (args.out / "steps.json").write_text(json.dumps(run.steps, indent=2))
    (args.out / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps({k: v for k, v in summary.items() if k != "latency_samples"}, indent=2))
    return 1 if run.errors or summary["steps_failed"] else 0


if __name__ == "__main__":
    raise SystemExit(main())
