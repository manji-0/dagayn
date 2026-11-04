"""Candidate pools for G8's four question kinds, from scip-go, gofuncs, and Go coverage.

    python pools.py INPUTS TESTIFY_TARGET COMPRESS_TARGET > pools.json

INPUTS holds build_inputs.sh's files for both repositories; each TARGET is
the checkout they were built on (the tag's archive, without a change).
Nothing here reads dagayn. PREREG.md states
each kind's rule; the code below is that rule.
"""

from __future__ import annotations

import json
import re
import sys
from collections import defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent.parent / "g7_go" / "keys"))
from per_test_cover import enclosing, load_funcs  # noqa: E402

# The G7 questions' targets, left out of every pool.
G7_TARGETS = {
    "assert/assertions.go::CollectT.Errorf",
    "assert/assertion_forward.go::Assertions.Equal",
    "suite/suite.go::Suite.T",
    "assert/assertions.go::ObjectsAreEqual",
    "flate/stateless.go::statelessEnc",
    "zstd/internal/xxhash/xxhash.go::Digest.Reset",
    "flate/deflate.go::compressor.storeFast",
    "flate/deflate.go::Writer.Reset",
}
# The coverage runs a test-reach key is read from (build_inputs.sh).
REACH_RUNS = ("cover1", "cover2")


class Index:
    """scip-go occurrences joined to the `func` declarations that hold them."""

    def __init__(self, inputs: Path, target: Path, name: str):
        self.target = target
        self.funcs = load_funcs(inputs / f"{name}.funcs.tsv")
        self.decls = {q: (rel, s, e) for rel, spans in self.funcs.items() for s, e, q in spans}
        self.defs: dict[str, str] = {}
        self.defined: set[str] = set()
        self.refs: dict[str, list[tuple[str, str, int, bool, str]]] = defaultdict(list)
        lines: dict[str, list[bytes]] = {}
        for line in (inputs / f"{name}.occ.tsv").read_text().splitlines():
            if line.startswith("#"):
                continue
            path, sl, _sc, el, ec, roles, symbol = line.split("\t")
            if symbol.startswith("local ") or skipped(path):
                continue
            fn = enclosing(self.funcs.get(path, []), int(sl))
            if int(roles) & 1:
                self.defined.add(symbol)
                if fn and symbol.endswith(")."):
                    self.defs[symbol] = fn
                continue
            if path not in lines:
                lines[path] = (target / path).read_bytes().splitlines()
            rest = lines[path][int(el) - 1][int(ec) :].lstrip()
            call = rest.startswith(b"(")
            text = lines[path][int(sl) - 1].decode(errors="replace").strip()
            self.refs[symbol].append((fn or f"{path}::<package level>", path, int(sl), call, text))
        self.symbol_of = {q: s for s, q in self.defs.items()}
        self.impl: dict[str, set[str]] = defaultdict(set)  # concrete method -> interface methods
        for line in (inputs / f"{name}.rels.tsv").read_text().splitlines():
            sym, rel, flags = line.split("\t")
            if "i" in flags and sym in self.defs:
                self.impl[sym].add(rel)
        self.implementors: dict[str, set[str]] = defaultdict(set)
        for sym, rels in self.impl.items():
            for r in rels:
                self.implementors[r].add(sym)

    def callers(self, symbol: str) -> set[str]:
        return {fn for fn, _p, _l, call, _t in self.refs.get(symbol, []) if call}

    def values(self, symbol: str) -> list[tuple[str, str, int, bool, str]]:
        return [r for r in self.refs.get(symbol, []) if not r[3]]

    def body(self, qname: str) -> str:
        rel, s, e = self.decls[qname]
        return "\n".join((self.target / rel).read_text().splitlines()[s - 1 : e])


def skipped(path: str) -> bool:
    """A file `go build ./...` leaves out: under testdata, `_x`, or `.x`."""
    return any(d == "testdata" or d[0] in "._" for d in path.split("/")[:-1])


def is_test_file(qname: str) -> bool:
    return qname.split("::")[0].endswith("_test.go")


def bare(qname: str) -> str:
    return qname.split("::")[1].rsplit(".", 1)[-1]


def reach(inputs: Path, name: str) -> dict[str, set[str]]:
    """Function -> the top-level `Test` functions that executed it in every run."""
    runs = [json.loads((inputs / f"{name}.{r}.json").read_text()) for r in REACH_RUNS]
    out: dict[str, set[str]] = defaultdict(set)
    tests = {t for c in runs for t in c["tests"] if t.split("::")[1].startswith("Test")}
    for t in tests:
        for fn in set.intersection(*(set(c["tests"].get(t, ())) for c in runs)):
            out[fn].add(t)
    return out


def interface_calls(ix: Index, symbol: str) -> set[str]:
    out: set[str] = set()
    for i in ix.impl.get(symbol, ()):
        out |= ix.callers(i)
    return out


def pools(inputs: Path, target: Path, name: str) -> dict:
    ix = Index(inputs, target, name)
    declared: dict[str, int] = defaultdict(int)
    for q in ix.decls:
        declared[bare(q)] += 1
    candidates = sorted(q for q in ix.symbol_of if q not in G7_TARGETS and not is_test_file(q))
    word = {q: re.compile(rf"\b{re.escape(bare(q))}\b") for q in candidates}

    # Kind 1: executed by 2 to 15 tests, fewer than half of which name it.
    executed = reach(inputs, name)
    k1 = []
    for q in candidates:
        tests = executed.get(q, set())
        if 2 <= len(tests) <= 15:
            naming = sum(bool(word[q].search(ix.body(t))) for t in tests if t in ix.decls)
            if naming * 2 < len(tests):
                k1.append({"target": q, "tests": len(tests), "tests_naming_it": naming})

    # Kinds 2 and 4 need a key that is the direct callers: no method value,
    # no call through an interface the method satisfies, no call outside a
    # function.
    def direct_only(q: str) -> bool:
        s = ix.symbol_of[q]
        return (
            not ix.values(s)
            and not interface_calls(ix, s)
            and not any("<package level>" in c for c in ix.callers(s))
        )

    # Kind 2: a method with 3 to 20 callers, whose name 3 or more `func`
    # declarations of the repository (functions or methods) share.
    k2 = []
    for q in candidates:
        if "." not in q.split("::")[1] or not direct_only(q):
            continue
        callers = ix.callers(ix.symbol_of[q])
        if declared[bare(q)] >= 3 and 3 <= len(callers) <= 20:
            k2.append({"target": q, "callers": len(callers), "declarations": declared[bare(q)]})

    # Kind 3a: the one method of the repository that implements a method of
    # an interface the repository declares, called through it at least once.
    # Kind 3b: a method stored as a value in a field or variable that is called.
    k3 = []
    for q in candidates:
        s = ix.symbol_of[q]
        through = {
            i
            for i in ix.impl.get(s, ())
            if i in ix.defined and ix.implementors[i] == {s} and ix.callers(i)
        }
        if through:
            k3.append({"target": q, "via": "interface", "interface_methods": sorted(through)})
            continue
        held = holders(ix, s)
        if held and all(ix.callers(h) for h in held):
            k3.append({"target": q, "via": "method value", "holders": sorted(held)})

    # Kind 4: 6 to 15 direct callers in 2 or more files, 2 or more outside tests.
    k4 = []
    for q in candidates:
        if not direct_only(q):
            continue
        callers = ix.callers(ix.symbol_of[q]) - {q}
        files = {c.split("::")[0] for c in callers}
        prod = sorted(c for c in callers if not is_test_file(c) and "<package level>" not in c)
        if 6 <= len(callers) <= 15 and len(files) >= 2 and len(prod) >= 2:
            k4.append({"target": q, "callers": len(callers), "non_test_callers": prod})
    return {"repository": name, "kind1": k1, "kind2": k2, "kind3": k3, "kind4": k4}


ASSIGN = re.compile(
    r"(?:^|[\s{,(])(\w+)\s*(?::?=|:)\s*(?:\(\*?\w+\)|\w+(?:\.\w+)*)\.\w+\s*[,})]?\s*$"
)


def holders(ix: Index, symbol: str) -> set[str]:
    """Fields or variables a method value of *symbol* is assigned to
    (`x.f = (*T).M`, `f := v.M`, `T{f: v.M}`), as scip-go symbols. Every other
    use (an argument, a return value) makes the pool skip the method: its value
    can flow anywhere."""
    out: set[str] = set()
    for _fn, path, line, _call, text in ix.values(symbol):
        m = ASSIGN.search(text)
        if not m:
            return set()
        name = m.group(1)
        found = {
            s
            for s, refs in ix.refs.items()
            if s.endswith((f"#{name}.", f"/{name}."))
            and any(p == path and n == line for _f, p, n, _c, _t in refs)
        }
        if not found:
            return set()
        out |= found
    return out


if __name__ == "__main__":
    inputs, targets = Path(sys.argv[1]), dict(zip(("testify", "compress"), sys.argv[2:4]))
    out = {name: pools(inputs, Path(t), name) for name, t in targets.items()}
    print(json.dumps(out, indent=1, sort_keys=True))
