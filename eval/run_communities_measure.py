#!/usr/bin/env python3
"""Measure ``architecture_analysis_tool`` communities against declared units.

For each measured clone: fetch ``overview`` (the units) and ``communities``
(every community), save the raw replies, and score the communities against
the real module boundaries.  A node's unit is the unit whose path is the
longest prefix of its file; its module is the first path component under the
unit (after a leading ``src/``).  Membership comes from the read-only graph
database, cross-checked against ``mode="community"`` with
``include_members`` for a few communities (a mismatch is an error).

Prints per repo: reply sizes, weighted purity against unit and module, the
share of community nodes in communities spanning more than one unit, how
many communities each unit is split into, duplicate and ``-subN`` names, and
an outline of the largest communities for hand checks.  When
``communities_judgments.yaml`` exists it refuses a judged id that is not a
community in that repository and tallies the verdicts.

    python eval/run_communities_measure.py --measure-dir DIR [--show 15]
"""

from __future__ import annotations

import argparse
import json
import re
import sqlite3
import subprocess
import sys
from collections import Counter, defaultdict
from pathlib import Path

import yaml

HERE = Path(__file__).resolve().parent
DAGAYN = HERE.parent / ".venv" / "bin" / "dagayn"
REPOS = ("cfwdon", "tstodon", "dagayn", "purecrate-ts")
JUDGMENTS = HERE / "communities_judgments.yaml"
VERDICTS = ("useful", "restates_unit", "incoherent")
DOC_KINDS = ("DocSection", "DocBody")


def call(repo: Path, **args: object) -> tuple[str, dict | None, str | None]:
    cmd = [str(DAGAYN), "tool", "architecture_analysis_tool", "--repo", str(repo)]
    for key, value in args.items():
        cmd += ["--arg", f"{key}={json.dumps(value)}"]
    try:
        proc = subprocess.run(cmd, capture_output=True, text=True, timeout=600)
    except subprocess.TimeoutExpired:
        return "", None, "timeout"
    try:
        reply = json.loads(proc.stdout)
    except json.JSONDecodeError:
        return proc.stdout, None, f"unparsable (rc={proc.returncode})"
    if reply.get("status") != "ok":
        return proc.stdout, reply, f"status={reply.get('status')}"
    return proc.stdout, reply, None


def unit_of(path: str, units: list[tuple[str, str]]) -> str:
    for prefix, name in units:
        if prefix in ("", ".") or path == prefix or path.startswith(prefix + "/"):
            return name
    return "(none)"


def module_of(path: str, unit_path: str | None) -> str:
    rel = path[len(unit_path) + 1 :] if unit_path and path.startswith(unit_path + "/") else path
    parts = rel.split("/")
    if parts[0] == "src" and len(parts) > 1:
        parts = parts[1:]
    return parts[0].split(".")[0] if len(parts) == 1 else parts[0]


def weighted_purity(groups: dict[int, Counter]) -> float:
    total = sum(sum(c.values()) for c in groups.values())
    return round(sum(max(c.values()) for c in groups.values()) / total, 3)


def score(rows: list[tuple], units: list[tuple[str, str]], unit_paths: dict[str, str]) -> dict:
    by_unit: dict[int, Counter] = defaultdict(Counter)
    by_module: dict[int, Counter] = defaultdict(Counter)
    for cid, _, path, _, _ in rows:
        unit = unit_of(path, units)
        by_unit[cid][unit] += 1
        by_module[cid][(unit, module_of(path, unit_paths.get(unit)))] += 1
    unit_sizes: Counter = Counter()
    module_sizes: Counter = Counter()
    for c in by_unit.values():
        unit_sizes.update(c)
    for c in by_module.values():
        module_sizes.update(c)
    n = len(rows)

    def multi(c: Counter) -> bool:
        ranked = c.most_common(2)
        return len(ranked) > 1 and ranked[1][1] >= 0.1 * sum(c.values())

    def cover(unit: str, share: float = 0.8) -> int:
        counts = sorted((c[unit] for c in by_unit.values() if c[unit]), reverse=True)
        done = 0
        for i, k in enumerate(counts, 1):
            done += k
            if done >= share * unit_sizes[unit]:
                return i
        return len(counts)

    return {
        "nodes": n,
        "purity_unit": weighted_purity(by_unit),
        "purity_unit_baseline_one_community": round(unit_sizes.most_common(1)[0][1] / n, 3),
        "purity_module": weighted_purity(by_module),
        "purity_module_baseline_one_community": round(module_sizes.most_common(1)[0][1] / n, 3),
        "modules": len(module_sizes),
        "share_in_any_multi_unit": round(
            sum(sum(c.values()) for c in by_unit.values() if len(c) > 1) / n, 3
        ),
        "share_in_multi_unit_10pct": round(
            sum(sum(c.values()) for c in by_unit.values() if multi(c)) / n, 3
        ),
        "per_unit": {
            u: {
                "nodes": k,
                "communities": sum(1 for c in by_unit.values() if c[u]),
                "cover80": cover(u),
            }
            for u, k in unit_sizes.most_common()
        },
        "_by_unit": by_unit,
        "_by_module": by_module,
    }


def measure(repo: Path, out: Path, show: int, errors: list[str]) -> dict:
    name = repo.name
    text, overview, err = call(repo, mode="overview", detail_level="standard")
    (out / f"{name}.overview.json").write_text(text)
    if err:
        errors.append(f"{name}/overview: {err}")
        return {}
    row: dict = {"overview_chars": len(text), "units": len(overview["units"])}
    for level in ("minimal", "standard"):
        text, listing, err = call(repo, mode="communities", top_n=100000, detail_level=level)
        (out / f"{name}.communities.{level}.json").write_text(text)
        if err:
            errors.append(f"{name}/communities/{level}: {err}")
            return row
        row[f"communities_{level}"] = {
            "chars": len(text),
            "listed": len(listing["communities"]),
            "total": listing.get("total"),
        }

    units = sorted(((u["path"], u["name"]) for u in overview["units"]), key=lambda u: -len(u[0]))
    unit_paths = {n: p for p, n in units}
    db = sqlite3.connect(f"file:{repo}/.dagayn/graph.db?mode=ro", uri=True)
    listed = db.execute(
        "SELECT name, level FROM communities WHERE id IN (SELECT community_id FROM nodes)"
    ).fetchall()
    names = Counter(n for n, _ in listed)
    row["communities"] = len(listed)
    row["communities_level_gt0"] = sum(1 for _, level in listed if level > 0)
    row["duplicate_names"] = sum(n for n in names.values() if n > 1)
    row["sub_split_names"] = sum(1 for n, _ in listed if re.search(r"-sub\d+$", n))
    rows = db.execute(
        "SELECT n.community_id, n.qualified_name, n.file_path, c.name, n.kind FROM nodes n "
        "JOIN communities c ON c.id = n.community_id"
    ).fetchall()
    row["nodes"] = db.execute("SELECT COUNT(*) FROM nodes").fetchone()[0]
    db.close()
    row["nodes_in_communities"] = len(rows)

    files: dict[int, Counter] = defaultdict(Counter)
    members: dict[int, set[str]] = defaultdict(set)
    cname: dict[int, str] = {}
    for cid, qn, path, comm, _ in rows:
        files[cid][path] += 1
        members[cid].add(qn)
        cname[cid] = comm

    largest = sorted(members, key=lambda c: -len(members[c]))
    for cid in largest[:3]:
        text, one, err = call(repo, mode="community", community_id=cid, include_members=True)
        if err:
            errors.append(f"{name}/community {cid}: {err}")
        elif set(one["community"]["members"]) != members[cid]:
            errors.append(f"{name}/community {cid}: tool members differ from the database")
    row["one_community_chars"] = len(text)

    row["all_nodes"] = score(rows, units, unit_paths)
    code = [r for r in rows if r[4] not in DOC_KINDS]
    row["code_nodes"] = score(code, units, unit_paths)
    hand = [r for r in code if "/generated/" not in r[2]]
    row["code_nodes_no_generated"] = score(hand, units, unit_paths)
    row["code_nodes_no_generated"].pop("_by_unit")
    row["code_nodes_no_generated"].pop("_by_module")
    row["code_nodes"].pop("_by_unit")
    row["code_nodes"].pop("_by_module")
    by_unit = row["all_nodes"].pop("_by_unit")
    by_module = row["all_nodes"].pop("_by_module")
    outline = []
    for cid in largest[:show]:
        size = len(members[cid])
        (top_unit, nu), (top_mod, nm) = (
            by_unit[cid].most_common(1)[0],
            by_module[cid].most_common(1)[0],
        )
        outline.append(
            {
                "id": cid,
                "name": cname[cid],
                "size": size,
                "unit": f"{top_unit} {nu / size:.2f}",
                "module": f"{top_mod[1]} {nm / size:.2f}",
                "units_spanned": len(by_unit[cid]),
                "top_files": [f"{p} ({n})" for p, n in files[cid].most_common(4)],
            }
        )
    row["largest"] = outline
    row["_ids"] = cname
    return row


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--measure-dir", type=Path, required=True)
    parser.add_argument("--show", type=int, default=15)
    args = parser.parse_args()
    out = args.measure_dir / "raw" / "communities"
    out.mkdir(parents=True, exist_ok=True)
    errors: list[str] = []
    summary = {name: measure(args.measure_dir / name, out, args.show, errors) for name in REPOS}

    if JUDGMENTS.exists():
        tally: dict[str, Counter] = defaultdict(Counter)
        for j in yaml.safe_load(JUDGMENTS.read_text())["judgments"]:
            if j["verdict"] not in VERDICTS:
                sys.exit(f"bad verdict {j['verdict']!r} for {j['repo']}/{j['id']}")
            if summary[j["repo"]]["_ids"].get(j["id"]) != j["name"]:
                sys.exit(f"judged community {j['id']} ({j['name']}) is not in {j['repo']}'s graph")
            tally[j["repo"]][j["verdict"]] += 1
            tally["ALL"][j["verdict"]] += 1
        summary["judged"] = {k: {**c, "n": sum(c.values())} for k, c in tally.items()}
    for row in summary.values():
        if isinstance(row, dict):
            row.pop("_ids", None)
    summary["errors"] = errors
    (out / "summary.json").write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary, indent=2))
    return 1 if errors else 0


if __name__ == "__main__":
    raise SystemExit(main())
