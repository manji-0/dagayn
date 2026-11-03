"""Where scip-go saw a symbol referenced, by enclosing function.

    python scip_refs.py OCC_TSV FUNCS_TSV SYMBOL_SUFFIX [SYMBOL_SUFFIX ...]

OCC_TSV is scipdump's output for the target's index, FUNCS_TSV gofuncs'
listing. Each SYMBOL_SUFFIX ends a scip-go symbol, such as
"`github.com/klauspost/compress/flate`/compressor#storeFast()." (a method)
or "`.../flate`/compressor#step." (a field). Prints one line per reference
occurrence (definitions left out): the enclosing function, then the site.
"""

from __future__ import annotations

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from per_test_cover import enclosing, load_funcs  # noqa: E402


def refs(occ: Path, funcs_tsv: Path, suffixes: list[str]) -> list[tuple[str, str, str]]:
    funcs = load_funcs(funcs_tsv)
    out = []
    for line in occ.read_text().splitlines():
        if line.startswith("#"):
            continue
        path, sl, sc, _el, _ec, roles, symbol = line.split("\t")
        if int(roles) & 1 or not any(symbol.endswith(s) for s in suffixes):
            continue
        fn = enclosing(funcs.get(path, []), int(sl)) or f"{path}::<package level>"
        out.append((fn, f"{path}:{sl}:{int(sc) + 1}", symbol.rsplit(" ", 1)[-1]))
    return out


def main() -> int:
    """With --repo DIR first, each line also shows the source line of the site."""
    args = sys.argv[1:]
    repo = None
    if args[0] == "--repo":
        repo, args = Path(args[1]), args[2:]
    occ, funcs_tsv, *suffixes = args
    for fn, site, symbol in refs(Path(occ), Path(funcs_tsv), suffixes):
        text = ""
        if repo is not None:
            path, line, _col = site.rsplit(":", 2)
            text = "\t" + (repo / path).read_text().splitlines()[int(line) - 1].strip()
        print(f"{fn}\t{site}\t{symbol.rsplit('/', 1)[-1]}{text}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
