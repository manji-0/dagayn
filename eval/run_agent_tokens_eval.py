#!/usr/bin/env python3
"""Whole-task eval: what an agent spends to answer with a dagayn build.

The contract tests (``crates/dagayn-tools/tests/tools.rs``) hold each reply to
its budget. What an agent pays is the whole task: every turn re-reads the
conversation, so an extra round trip costs more than a long reply
(docs/plans/AGENT-WORKFLOW-TARGET.md#order-of-work, step 10). This harness
runs Claude Code headless on fixed questions about dagayn 4.15.0 and compares
two setups, each a dagayn command that builds the graph and serves MCP, or
``none`` for no MCP server at all.

For each run it records the answer's F1 and exact match against the answer
key in ``eval/agent_tokens_tasks.json`` (or ``--tasks-file``), total tokens (input, output, cache
read and write, summed over models), turns, tool calls, dagayn calls, and the
size of each dagayn reply. It reports, per setup, the geometric-mean token
ratio against the baseline, median turns, dagayn calls per run, replies over
8,000 characters, and exact matches.

It needs the ``claude`` CLI and API access, so it is a manual gate, not CI.
Claude Haiku 5.5 costs about 1 USD per 100 runs.

Usage::

    # The working tree's build against the release on PyPI, 2 reps per task.
    # Commands run inside the target checkout, so give paths, not `uv run`.
    uv run python eval/run_agent_tokens_eval.py \\
        --candidate "$PWD/.venv/bin/dagayn" \\
        --baseline "uvx --from dagayn==9.0.0 dagayn" --reps 2
    # Against no MCP server, with a gate on the token ratio.
    uv run python eval/run_agent_tokens_eval.py --candidate dagayn --baseline none \\
        --reps 10 --gate --max-token-ratio 1.2

The target is ``git archive <source_commit>`` of this repository, made into a
one-commit repository; fetch the commit first in a shallow clone.
"""

from __future__ import annotations

import argparse
import csv
import json
import math
import os
import random
import re
import shlex
import shutil
import statistics
import subprocess
import sys
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parent.parent
TASKS_FILE = Path(__file__).with_name("agent_tokens_tasks.json")
JSON_BLOCK = re.compile(r"```json\s*(\{.*?\})\s*```", re.S)
# Subagents run on another model and hide their turns; the web is irrelevant.
DISALLOWED = "Agent,Task,WebSearch,WebFetch"
ALLOWED = "Bash,Read,Grep,Glob,Write,Edit,TodoWrite,mcp__dagayn"
# Only what the CLI needs to reach the API.
PASS_ENV = (
    "ANTHROPIC_API_KEY",
    "ANTHROPIC_BASE_URL",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
    "https_proxy",
    "http_proxy",
    "no_proxy",
    "NODE_EXTRA_CA_CERTS",
    "SSL_CERT_FILE",
    "LANG",
)
REPLY_BUDGET = 8000


def load_tasks(path: Path = TASKS_FILE) -> dict[str, Any]:
    return json.loads(path.read_text(encoding="utf-8"))


def prompt(spec: dict[str, Any], task: str, graph: bool) -> str:
    head, sep, tail = spec["preamble"].partition("- 回答の最後に")
    line = spec["condition_line"]["graph" if graph else "none"]
    return head + line + sep + tail + spec["tasks"][task]["prompt"]


def norm_path(item: str) -> str:
    item = str(item).strip().strip("`")
    return item[2:] if item.startswith("./") else item


def canon_cycle(cycle: list[str]) -> tuple[str, ...]:
    nodes = [norm_path(x).rstrip("/") for x in cycle]
    if len(nodes) > 1 and nodes[0] == nodes[-1]:
        nodes = nodes[:-1]
    i = nodes.index(min(nodes)) if nodes else 0
    return tuple(nodes[i:] + nodes[:i])


def answer_set(kind: str, items: list[Any]) -> set[Any]:
    if kind == "cycles":
        return {canon_cycle(c) for c in items if isinstance(c, list)}
    return {norm_path(x) for x in items if isinstance(x, str)}


def make_target(work: Path, commit: str) -> Path:
    src = work / "src"
    if src.exists():
        return src
    src.mkdir(parents=True)
    archive = subprocess.run(
        ["git", "-C", str(REPO_ROOT), "archive", commit], capture_output=True, check=True
    ).stdout
    subprocess.run(["tar", "-x", "-C", str(src)], input=archive, check=True)
    git = ["git", "-c", "user.name=eval", "-c", "user.email=eval@example.invalid"]
    subprocess.run(["git", "init", "-q"], cwd=src, check=True)
    subprocess.run(["git", "add", "-A"], cwd=src, check=True)
    subprocess.run([*git, "commit", "-q", "-m", f"archive {commit[:12]}"], cwd=src, check=True)
    return src


def clean_path() -> str:
    """PATH without any directory that holds a dagayn executable."""
    keep = [
        d for d in os.environ.get("PATH", "").split(":") if d and not (Path(d) / "dagayn").exists()
    ]
    return ":".join(keep)


def mcp_config(work: Path, name: str, command: str) -> Path:
    argv = shlex.split(command)
    exe = shutil.which(argv[0]) or argv[0]
    cfg = work / f"mcp-{name}.json"
    server = {"type": "stdio", "command": exe, "args": [*argv[1:], "serve"]}
    cfg.write_text(json.dumps({"mcpServers": {"dagayn": server}}))
    return cfg


def run_one(item: dict[str, Any], ctx: dict[str, Any]) -> dict[str, Any]:
    raw: Path = ctx["raw"]
    meta_path = raw / f"{item['id']}.meta.json"
    if meta_path.exists():
        return json.loads(meta_path.read_text())
    work: Path = ctx["work"]
    repo = work / "runs" / item["id"]
    shutil.rmtree(repo, ignore_errors=True)
    subprocess.run(["git", "clone", "-q", str(ctx["src"]), str(repo)], check=True)
    command = ctx["setups"][item["setup"]]
    graph = command != "none"
    meta: dict[str, Any] = {**item, "model": ctx["model"]}
    if graph:
        t0 = time.time()
        subprocess.run(
            [*shlex.split(command), "build", "--local-embedding", "none"],
            cwd=repo,
            capture_output=True,
            check=True,
        )
        meta["graph_build_s"] = round(time.time() - t0, 2)
    home = work / "home" / item["id"]
    home.mkdir(parents=True, exist_ok=True)
    env = {k: os.environ[k] for k in PASS_ENV if k in os.environ}
    env.update(HOME=str(home), PATH=clean_path())
    cmd = [
        "claude", "-p", "--output-format", "stream-json", "--verbose",
        "--model", ctx["model"], "--effort", ctx["effort"],
        "--allowedTools", ALLOWED, "--disallowedTools", DISALLOWED, "--strict-mcp-config",
    ]  # fmt: skip
    if graph:
        cmd += ["--mcp-config", str(ctx["mcp"][item["setup"]])]
    cmd += ["--", prompt(ctx["spec"], item["task"], graph)]
    t0 = time.monotonic()
    with open(raw / f"{item['id']}.jsonl", "w") as out:
        try:
            p = subprocess.run(
                cmd, cwd=repo, env=env, stdout=out, stderr=subprocess.DEVNULL,
                stdin=subprocess.DEVNULL, timeout=ctx["timeout"],
            )  # fmt: skip
            meta.update(exit_code=p.returncode, timed_out=False)
        except subprocess.TimeoutExpired:
            meta.update(exit_code=None, timed_out=True)
    meta["wall_s"] = round(time.monotonic() - t0, 2)
    meta_path.write_text(json.dumps(meta, indent=1) + "\n")
    shutil.rmtree(repo, ignore_errors=True)
    shutil.rmtree(home, ignore_errors=True)
    print(f"{item['id']} done in {meta['wall_s']}s", flush=True)
    return meta


def result_text(block: dict[str, Any]) -> str:
    content = block.get("content")
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "".join(x.get("text", "") for x in content if isinstance(x, dict))
    return ""


def score(meta: dict[str, Any], raw: Path, spec: dict[str, Any]) -> dict[str, Any]:
    names: dict[str, str] = {}
    tools: list[str] = []
    replies: list[int] = []
    result: dict[str, Any] = {}
    for line in (raw / f"{meta['id']}.jsonl").read_text().splitlines():
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        kind = event.get("type")
        if kind == "assistant":
            for block in event.get("message", {}).get("content", []):
                if block.get("type") == "tool_use":
                    names[block["id"]] = block["name"]
                    tools.append(block["name"])
        elif kind == "user":
            content = event.get("message", {}).get("content", [])
            for block in content if isinstance(content, list) else []:
                if block.get("type") == "tool_result" and names.get(
                    block.get("tool_use_id"), ""
                ).startswith("mcp__"):
                    replies.append(len(result_text(block)))
        elif kind == "result":
            result = event
    task = spec["tasks"][meta["task"]]
    truth = answer_set(task["kind"], task["truth"])
    blocks = JSON_BLOCK.findall(result.get("result", "") or "")
    parsed = None
    if blocks:
        try:
            parsed = json.loads(blocks[-1]).get("answer")
        except (json.JSONDecodeError, AttributeError):
            parsed = None
    answer = answer_set(task["kind"], parsed) if isinstance(parsed, list) else set()
    tp = len(answer & truth)
    precision = tp / len(answer) if answer else 0.0
    recall = tp / len(truth)
    f1 = 2 * precision * recall / (precision + recall) if precision + recall else 0.0
    usage = result.get("modelUsage") or {}
    keys = ("inputTokens", "outputTokens", "cacheReadInputTokens", "cacheCreationInputTokens")
    work_tools = [t for t in tools if t not in ("TodoWrite", "ToolSearch")]
    return {
        "id": meta["id"],
        "setup": meta["setup"],
        "task": meta["task"],
        "ok": bool(result) and not result.get("is_error") and not meta.get("timed_out"),
        "f1": round(f1, 4),
        "exact": int(answer == truth),
        "total_tokens": sum(m.get(k, 0) or 0 for m in usage.values() for k in keys),
        "cost_usd": round(result.get("total_cost_usd", 0) or 0, 4),
        "duration_s": round((result.get("duration_ms") or 0) / 1000, 2),
        "turns": result.get("num_turns") or 0,
        "tool_calls": len(work_tools),
        "dagayn_calls": sum(t.startswith("mcp__") for t in work_tools),
        "dagayn_reply_max": max(replies, default=0),
        "dagayn_replies_over_budget": sum(n > REPLY_BUDGET for n in replies),
        "dagayn_tools": " ".join(
            t.removeprefix("mcp__dagayn__") for t in work_tools if t.startswith("mcp__")
        ),
    }


def summarize(rows: list[dict[str, Any]], baseline: str) -> dict[str, Any]:
    tasks = sorted({r["task"] for r in rows})
    out: dict[str, Any] = {}
    for setup in sorted({r["setup"] for r in rows}):
        mine = [r for r in rows if r["setup"] == setup]
        ratios = []
        for task in tasks:
            a = [r["total_tokens"] for r in mine if r["task"] == task and r["total_tokens"] > 0]
            b = [
                r["total_tokens"]
                for r in rows
                if r["setup"] == baseline and r["task"] == task and r["total_tokens"] > 0
            ]
            if a and b:
                ratios.append(statistics.mean(map(math.log, a)) - statistics.mean(map(math.log, b)))
        out[setup] = {
            "runs": len(mine),
            "failed": sum(not r["ok"] for r in mine),
            "token_ratio_vs_baseline": round(math.exp(statistics.mean(ratios)), 3)
            if ratios
            else None,
            "median_turns": statistics.median(r["turns"] for r in mine),
            "median_dagayn_calls": statistics.median(r["dagayn_calls"] for r in mine),
            "replies_over_8000": sum(r["dagayn_replies_over_budget"] for r in mine),
            "exact": sum(r["exact"] for r in mine),
            "mean_f1": round(statistics.mean(r["f1"] for r in mine), 4),
            "cost_usd": round(sum(r["cost_usd"] for r in mine), 2),
        }
    return out


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--candidate", required=True, help='dagayn command for setup "candidate"')
    ap.add_argument(
        "--baseline", default="none", help='dagayn command for setup "baseline", or "none"'
    )
    ap.add_argument("--model", default="claude-haiku-5-5")
    ap.add_argument("--effort", default="medium")
    ap.add_argument("--reps", type=int, default=10)
    ap.add_argument("--tasks", default="", help="comma-separated task ids (default: all)")
    ap.add_argument("--tasks-file", type=Path, default=TASKS_FILE, help="questions and answers")
    ap.add_argument("-j", type=int, default=3)
    ap.add_argument("--seed", type=int, default=20261010)
    ap.add_argument("--timeout", type=int, default=900)
    ap.add_argument(
        "--out", type=Path, default=None, help="keeps raw runs; reruns skip finished ids"
    )
    ap.add_argument("--gate", action="store_true")
    ap.add_argument("--max-token-ratio", type=float, default=1.0)
    args = ap.parse_args()

    spec = load_tasks(args.tasks_file)
    tasks = args.tasks.split(",") if args.tasks else list(spec["tasks"])
    setups = {"candidate": args.candidate, "baseline": args.baseline}
    out = args.out or Path(tempfile.mkdtemp(prefix="dagayn-agent-tokens-"))
    raw = out / "raw"
    raw.mkdir(parents=True, exist_ok=True)
    work = Path(tempfile.mkdtemp(prefix="dagayn-agent-tokens-work-"))
    graphs = {name: cmd for name, cmd in setups.items() if cmd != "none"}
    try:
        ctx = {
            "spec": spec, "setups": setups, "model": args.model, "effort": args.effort,
            "timeout": args.timeout, "raw": raw, "work": work,
            "src": make_target(work, spec["source_commit"]),
            "mcp": {name: mcp_config(work, name, cmd) for name, cmd in graphs.items()},
        }  # fmt: skip
        items = [
            {"task": t, "setup": s, "rep": r}
            for t in tasks
            for s in setups
            for r in range(args.reps)
        ]
        random.Random(args.seed).shuffle(items)
        for i, item in enumerate(items):
            item["id"] = f"{i:03d}-{item['task']}-{item['setup']}"
        with ThreadPoolExecutor(max_workers=args.j) as pool:
            metas = list(pool.map(lambda it: run_one(it, ctx), items))
    finally:
        shutil.rmtree(work, ignore_errors=True)

    rows = [score(m, raw, spec) for m in metas]
    with open(out / "results.csv", "w", newline="") as fh:
        writer = csv.DictWriter(fh, fieldnames=list(rows[0]))
        writer.writeheader()
        writer.writerows(rows)
    summary = summarize(rows, "baseline")
    (out / "summary.json").write_text(json.dumps(summary, indent=1) + "\n")
    print(json.dumps(summary, indent=1))
    print(f"results: {out}")
    if args.gate:
        ratio = summary["candidate"]["token_ratio_vs_baseline"]
        failed = summary["candidate"]["failed"]
        if failed or ratio is None or ratio > args.max_token_ratio:
            limit = args.max_token_ratio
            print(f"gate failed: token ratio {ratio} (max {limit}), {failed} failed runs")
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
