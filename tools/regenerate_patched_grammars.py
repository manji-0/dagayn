"""Regenerate the parsers of patched grammars (docs/GRAMMAR-PROVISIONING.md).

For each language with patches under `vendor/grammar-patches/<language>/`:

1. fetch the pinned upstream source;
2. regenerate it unpatched with the tree-sitter CLI version its
   `package-lock.json` records, and require the result to match the
   upstream `parser.c` (CLI versions generate different parsers, so this
   proves the CLI reproduces upstream);
3. apply the patches in name order, regenerate, and run `tree-sitter test`;
4. write the required files that changed under `vendor/grammars/<language>/`
   (`parser.c` gzipped) with a `STAMP.json` naming the pin and patches.

Builds only copy those files over the fetched upstream source; they need
neither the CLI nor node. `--check` verifies the stamps without network.

Needs `git` (for `git apply`) and `npm` (to install the CLI).
"""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from dagayn import vendor_grammars  # noqa: E402

# Files generated this large are committed gzipped.
GZIP_THRESHOLD = 1 << 20
# Generated lines that carry the grammar package's own version, which
# upstream may bump without regenerating.
VERSION_LINE = re.compile(rb"^\s*\.(major|minor|patch)_version = \d+,$", re.M)


def patched_languages() -> list[str]:
    root = vendor_grammars.VENDOR_ROOT / "grammar-patches"
    if not root.is_dir():
        return []
    return sorted(path.name for path in root.iterdir() if path.is_dir())


def fetch_upstream(spec: vendor_grammars.GrammarSpec, destination: Path) -> None:
    archive = destination.parent / f"{destination.name}.tar.gz"
    vendor_grammars._download_archive(spec, archive)
    vendor_grammars._extract_archive(
        archive, destination, source_subdirectory=spec.source_subdirectory
    )


def cli_version(source: Path) -> str:
    lock = json.loads((source / "package-lock.json").read_text(encoding="utf-8"))
    return lock["packages"]["node_modules/tree-sitter-cli"]["version"]


def install_cli(version: str, cache_root: Path) -> Path:
    prefix = cache_root / f"tree-sitter-cli-{version}"
    binary = prefix / "node_modules" / ".bin" / "tree-sitter"
    if not binary.exists():
        subprocess.run(
            ["npm", "install", "--prefix", str(prefix), f"tree-sitter-cli@{version}"],
            check=True,
            stdout=subprocess.DEVNULL,
        )
    return binary


def parser_root(spec: vendor_grammars.GrammarSpec, source: Path) -> Path:
    return source / spec.parser_subdirectory if spec.parser_subdirectory else source


def generate(cli: Path, spec: vendor_grammars.GrammarSpec, source: Path) -> None:
    subprocess.run([str(cli), "generate"], cwd=parser_root(spec, source), check=True)


def is_runtime_header(rel_path: str) -> bool:
    # `generate` rewrites src/tree_sitter/*.h from the CLI; upstream may ship
    # older copies, which its parser.c (same CLI version) builds against.
    return "tree_sitter" in Path(rel_path).parts


def without_version_lines(content: bytes) -> bytes:
    return VERSION_LINE.sub(b"", content)


def regenerate(language: str, cli_override: str | None) -> None:
    spec = vendor_grammars.GRAMMAR_SPECS[language]
    patches = vendor_grammars.grammar_patch_files(language)
    digest = vendor_grammars.grammar_patch_digest(language)
    if digest is None:
        raise SystemExit(f"{language}: no patches under vendor/grammar-patches/{language}")
    cache_root = vendor_grammars.get_grammar_cache_root()
    cache_root.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=f"{language}-regenerate-") as tmp:
        tmp_root = Path(tmp)
        upstream = tmp_root / "upstream"
        fetch_upstream(spec, upstream)
        version = cli_override or cli_version(upstream)
        cli = install_cli(version, cache_root)

        print(f"{language}: checking that tree-sitter {version} reproduces upstream")
        reproduced = tmp_root / "reproduced"
        shutil.copytree(upstream, reproduced)
        generate(cli, spec, reproduced)
        for rel_path in spec.required_paths:
            if Path(rel_path).name != "parser.c":
                continue
            expected = without_version_lines((upstream / rel_path).read_bytes())
            actual = without_version_lines((reproduced / rel_path).read_bytes())
            if expected != actual:
                raise SystemExit(
                    f"{language}: tree-sitter {version} does not reproduce upstream {rel_path}; "
                    "pass --cli-version with the version upstream generated it with"
                )

        print(f"{language}: applying {len(patches)} patch(es)")
        patched = tmp_root / "patched"
        shutil.copytree(upstream, patched)
        for patch in patches:
            subprocess.run(
                ["git", "apply", "--whitespace=nowarn", str(patch)], cwd=patched, check=True
            )
        generate(cli, spec, patched)
        subprocess.run([str(cli), "test"], cwd=parser_root(spec, patched), check=True)

        output = vendor_grammars.get_patched_grammar_dir(language)
        if output.exists():
            shutil.rmtree(output)
        output.mkdir(parents=True)
        files: dict[str, str] = {}
        for rel_path in spec.required_paths:
            source = patched / rel_path
            if not source.exists() or is_runtime_header(rel_path):
                continue
            content = source.read_bytes()
            original = upstream / rel_path
            if original.exists() and original.read_bytes() == content:
                continue
            target = output / rel_path
            target.parent.mkdir(parents=True, exist_ok=True)
            if len(content) >= GZIP_THRESHOLD:
                # mtime=0 keeps the archive identical across regenerations.
                target.with_name(f"{target.name}.gz").write_bytes(
                    gzip.compress(content, compresslevel=9, mtime=0)
                )
            else:
                target.write_bytes(content)
            files[rel_path] = hashlib.sha256(content).hexdigest()
        stamp = {
            "language": language,
            "upstream": f"{spec.owner}/{spec.repo}",
            "commit": spec.commit,
            "patches": [patch.name for patch in patches],
            "patches_sha256": digest,
            "tree_sitter_cli": version,
            "files": files,
        }
        (output / vendor_grammars.STAMP_NAME).write_text(
            json.dumps(stamp, indent=2) + "\n", encoding="utf-8"
        )
        print(f"{language}: wrote {', '.join(files)} to {output.relative_to(ROOT)}")


def check(languages: list[str]) -> int:
    problems = [
        problem
        for language in languages
        for problem in vendor_grammars.check_patched_grammar(language)
    ]
    for problem in problems:
        print(problem, file=sys.stderr)
    return 1 if problems else 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("languages", nargs="*", help="default: every patched grammar")
    parser.add_argument("--check", action="store_true", help="only verify the stamps")
    parser.add_argument("--cli-version", help="tree-sitter CLI version to generate with")
    args = parser.parse_args()
    languages = args.languages or patched_languages()
    if args.check:
        stamped = vendor_grammars.VENDOR_ROOT / "grammars"
        if stamped.is_dir():
            languages = sorted({*languages, *(path.name for path in stamped.iterdir())})
        return check(languages)
    for language in languages:
        regenerate(language, args.cli_version)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
