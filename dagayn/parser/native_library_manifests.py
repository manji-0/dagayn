"""C / C++ shared libraries declared by build systems.

A Python ``ctypes.CDLL("libfastsum.so")``, a Java ``System.loadLibrary``,
or a C# ``NativeLibrary.Load`` names a build output, not a source file. This
module reads the build files that produce such outputs and reports which
repository sources each shared library is compiled from:

* CMake: ``add_library(NAME SHARED|MODULE ...)`` (or a plain
  ``add_library`` when ``BUILD_SHARED_LIBS`` is on), ``target_sources``,
  ``set`` / ``list(APPEND)`` / ``file(GLOB)`` variables, and the
  ``OUTPUT_NAME`` target property;
* Meson: ``shared_library``, ``shared_module``, ``both_libraries``, and
  ``library`` unless ``default_library`` is ``static``;
* node-gyp: ``binding.gyp`` targets, whose ``sources`` build the Node.js
  addon ``build/Release/<target_name>.node``;
* Cargo build scripts: ``cc::Build::new().file(...).compile("name")`` and
  ``cxx_build::bridge(...)`` chains in ``build.rs``, whose sources are
  linked into the crate beside them;
* Emscripten: ``emcc`` / ``em++`` command lines, whose ``-o`` names the
  JavaScript glue and ``.wasm`` module and whose ``-sEXPORTED_FUNCTIONS``
  lists the C functions JavaScript may call;
* compiler command lines with ``-shared`` / ``-dynamiclib`` / ``-bundle``
  in Makefile recipes (``$@`` / ``$^`` / ``$<`` and simple variables
  expanded, object files mapped back to their sources), justfiles, and
  package.json scripts.

Only sources that exist in the repository count; a library none of whose
sources can be located is not reported.
"""

from __future__ import annotations

import ast
import re
from dataclasses import dataclass, field
from pathlib import Path, PurePosixPath
from typing import Iterator

from .manifest_bridges import _command_options, _resolve_rel, _split_command

C_SOURCE_SUFFIXES = (".c", ".cc", ".cpp", ".cxx", ".c++", ".m", ".mm")
_CPP_SUFFIXES = (".cc", ".cpp", ".cxx", ".c++", ".mm")
_SHARED_OUTPUT_RE = re.compile(r"\.(?:so(?:\.\d+)*|dylib|dll|bundle)$")


@dataclass
class NativeLibrary:
    """A shared library a build file compiles from repository sources."""

    config_rel: str
    # "cmake" | "meson" | "make" | "node-gyp" | "emscripten" | "cc" | "cxx"
    build_system: str
    lib_name: str
    sources: list[str] = field(default_factory=list)
    # Build outputs at known paths (node-gyp's `build/Release/NAME.node`,
    # Emscripten's glue and `.wasm`).
    outputs: list[str] = field(default_factory=list)
    # Emscripten `EXPORTED_FUNCTIONS` without the leading `_`; `None` when
    # the command does not list them.
    wasm_exports: list[str] | None = None

    @property
    def language(self) -> str:
        if any(source.endswith(_CPP_SUFFIXES) for source in self.sources):
            return "cpp"
        if any(source.endswith(".m") for source in self.sources):
            return "objc"
        return "c"


def library_stem(output: str) -> str:
    """``NAME`` from ``build/libNAME.so.1``, the form loaders are matched by."""
    base = output.replace("\\", "/").rsplit("/", 1)[-1]
    base = _SHARED_OUTPUT_RE.sub("", base)
    base = base.removeprefix("lib")
    return base.replace("-", "_")


def _is_source(path: str) -> bool:
    return path.endswith(C_SOURCE_SUFFIXES)


def _existing_sources(repo_root: Path, base: PurePosixPath, items: list[str]) -> list[str]:
    sources: list[str] = []
    for item in items:
        if not _is_source(item) or "$" in item:
            continue
        rel = _resolve_rel(base, item)
        if rel is not None and (repo_root / rel).is_file() and rel not in sources:
            sources.append(rel)
    return sources


# ---------------------------------------------------------------------------
# CMake
# ---------------------------------------------------------------------------

_CMAKE_NAME_RE = re.compile(r"[A-Za-z_][A-Za-z0-9_]*")
_CMAKE_VAR_RE = re.compile(r"\$\{([A-Za-z0-9_.+-]+)\}")
_CMAKE_LIBRARY_TYPES = {"STATIC", "SHARED", "MODULE", "OBJECT", "INTERFACE", "UNKNOWN"}
_CMAKE_SOURCE_KEYWORDS = {"INTERFACE", "PUBLIC", "PRIVATE", "EXCLUDE_FROM_ALL"}
_CMAKE_TRUE = {"ON", "YES", "TRUE", "Y", "1"}


def _cmake_strip_comments(text: str) -> str:
    text = re.sub(r"#\[(=*)\[.*?\]\1\]", "", text, flags=re.S)
    out: list[str] = []
    for line in text.splitlines():
        quoted = False
        for index, char in enumerate(line):
            if char == '"' and (index == 0 or line[index - 1] != "\\"):
                quoted = not quoted
            elif char == "#" and not quoted:
                line = line[:index]
                break
        out.append(line)
    return "\n".join(out)


def _cmake_commands(text: str) -> Iterator[tuple[str, list[tuple[str, bool]]]]:
    """``(command, [(argument, quoted), ...])`` in source order."""
    text = _cmake_strip_comments(text)
    pos = 0
    while True:
        match = _CMAKE_NAME_RE.search(text, pos)
        if match is None:
            return
        after = match.end()
        while after < len(text) and text[after] in " \t":
            after += 1
        if after >= len(text) or text[after] != "(":
            pos = match.end()
            continue
        args: list[tuple[str, bool]] = []
        depth, index, current = 1, after + 1, ""
        while index < len(text) and depth:
            char = text[index]
            if char == '"':
                end = index + 1
                while end < len(text) and text[end] != '"':
                    end += 2 if text[end] == "\\" else 1
                args.append((text[index + 1 : end], True))
                index = end + 1
                continue
            if char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
            if char.isspace() or char in "()":
                if current:
                    args.append((current, False))
                    current = ""
            else:
                current += char
            index += 1
        yield match.group(0).lower(), args
        pos = index


@dataclass
class _CMakeTarget:
    name: str
    shared: bool
    output_name: str | None = None
    items: list[str] = field(default_factory=list)


def _cmake_libraries(repo_root: Path, cmake_rel: str) -> list[NativeLibrary]:
    try:
        text = (repo_root / cmake_rel).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    base = PurePosixPath(cmake_rel).parent
    at_root = str(base) in ("", ".")
    commands = list(_cmake_commands(text))
    has_project = any(name == "project" for name, _ in commands)
    variables: dict[str, list[str]] = {
        "CMAKE_CURRENT_SOURCE_DIR": ["."],
        "CMAKE_CURRENT_LIST_DIR": ["."],
    }
    if has_project:
        variables["PROJECT_SOURCE_DIR"] = ["."]
    if at_root:
        variables["CMAKE_SOURCE_DIR"] = ["."]

    def expand(arg: str, quoted: bool) -> list[str]:
        whole = _CMAKE_VAR_RE.fullmatch(arg)
        if whole is not None and not quoted:
            return list(variables.get(whole.group(1), [arg]))
        value = _CMAKE_VAR_RE.sub(
            lambda m: ";".join(variables[m.group(1)]) if m.group(1) in variables else m.group(0),
            arg,
        )
        if quoted:
            return [value]
        return [part for part in value.split(";") if part]

    def values(args: list[tuple[str, bool]]) -> list[str]:
        return [value for arg, quoted in args for value in expand(arg, quoted)]

    shared_default = False
    targets: dict[str, _CMakeTarget] = {}
    for command, raw in commands:
        if not raw:
            continue
        args = values(raw)
        if not args:
            continue
        if command == "set":
            var, rest = args[0], args[1:]
            for stop in ("CACHE", "PARENT_SCOPE"):
                if stop in rest:
                    rest = rest[: rest.index(stop)]
            variables[var] = rest
            if var == "BUILD_SHARED_LIBS":
                shared_default = bool(rest) and rest[0].upper() in _CMAKE_TRUE
        elif command == "option" and args[0] == "BUILD_SHARED_LIBS":
            shared_default = len(args) >= 3 and args[2].upper() in _CMAKE_TRUE
        elif command == "list" and len(args) >= 2 and args[0] == "APPEND":
            variables.setdefault(args[1], []).extend(args[2:])
        elif command == "file" and len(args) >= 3 and args[0] in ("GLOB", "GLOB_RECURSE"):
            variables[args[1]] = _cmake_glob(repo_root, base, args[0], args[2:])
        elif command == "add_library":
            name, rest = args[0], args[1:]
            if rest and rest[0] in ("ALIAS", "IMPORTED"):
                continue
            kind = rest[0] if rest and rest[0] in _CMAKE_LIBRARY_TYPES else None
            if kind is not None:
                rest = rest[1:]
            shared = kind in ("SHARED", "MODULE") or (kind is None and shared_default)
            target = targets.setdefault(name, _CMakeTarget(name, shared))
            target.shared = shared
            target.items.extend(item for item in rest if item not in _CMAKE_SOURCE_KEYWORDS)
        elif command == "target_sources" and args[0] in targets:
            targets[args[0]].items.extend(
                item for item in args[1:] if item not in _CMAKE_SOURCE_KEYWORDS
            )
        elif command == "set_target_properties" and "PROPERTIES" in args:
            split = args.index("PROPERTIES")
            props = args[split + 1 :]
            for key, value in zip(props[::2], props[1::2]):
                if key in ("OUTPUT_NAME", "LIBRARY_OUTPUT_NAME"):
                    for name in args[:split]:
                        if name in targets:
                            targets[name].output_name = value

    libraries: list[NativeLibrary] = []
    for target in targets.values():
        if not target.shared:
            continue
        sources = _existing_sources(repo_root, base, target.items)
        if sources:
            name = (target.output_name or target.name).replace("-", "_")
            libraries.append(NativeLibrary(cmake_rel, "cmake", name, sources))
    return libraries


def _cmake_glob(repo_root: Path, base: PurePosixPath, mode: str, args: list[str]) -> list[str]:
    patterns: list[str] = []
    skip_next = False
    for arg in args:
        if skip_next:
            skip_next = False
            continue
        if arg in ("LIST_DIRECTORIES", "RELATIVE"):
            skip_next = True
            continue
        if arg == "CONFIGURE_DEPENDS":
            continue
        patterns.append(arg)
    directory = repo_root / base
    found: list[str] = []
    for pattern in patterns:
        pattern = pattern.removeprefix("./")
        if pattern.startswith(("/", "..")) or "$" in pattern:
            continue
        if mode == "GLOB_RECURSE":
            parent, _, leaf = pattern.rpartition("/")
            matches = (directory / parent).rglob(leaf) if parent else directory.rglob(leaf)
        else:
            matches = directory.glob(pattern)
        for path in sorted(matches):
            if path.is_file():
                found.append(path.relative_to(directory).as_posix())
    return found


# ---------------------------------------------------------------------------
# Meson
# ---------------------------------------------------------------------------

_MESON_STRING_RE = re.compile(r"'((?:[^'\\]|\\.)*)'")
_MESON_ASSIGN_RE = re.compile(r"^\s*([A-Za-z_]\w*)\s*(\+?=)\s*", re.M)
_MESON_LIBRARY_RE = re.compile(r"\b(shared_library|shared_module|both_libraries|library)\s*\(")


def _meson_balanced(text: str, start: int) -> int:
    """Index just past the bracket expression opening at *start*."""
    depth = 0
    index = start
    while index < len(text):
        char = text[index]
        if char == "'":
            match = _MESON_STRING_RE.match(text, index)
            index = match.end() if match else index + 1
            continue
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
            if depth == 0:
                return index + 1
        index += 1
    return index


def _meson_split_args(text: str) -> list[str]:
    parts: list[str] = []
    depth, current, index = 0, "", 0
    while index < len(text):
        char = text[index]
        if char == "'":
            match = _MESON_STRING_RE.match(text, index)
            end = match.end() if match else index + 1
            current += text[index:end]
            index = end
            continue
        if char in "([{":
            depth += 1
        elif char in ")]}":
            depth -= 1
        if char == "," and depth == 0:
            parts.append(current.strip())
            current = ""
        else:
            current += char
        index += 1
    if current.strip():
        parts.append(current.strip())
    return parts


def _meson_values(expr: str, variables: dict[str, list[str]]) -> list[str]:
    """String literals in *expr*, with bare identifiers replaced by their lists."""
    values: list[str] = []
    stripped = _MESON_STRING_RE.sub(lambda m: (values.append(m.group(1)), " ")[1], expr)
    for name in re.findall(r"\b[A-Za-z_]\w*\b", stripped):
        values.extend(variables.get(name, []))
    return values


def _meson_libraries(repo_root: Path, meson_rel: str) -> list[NativeLibrary]:
    try:
        text = (repo_root / meson_rel).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    text = "\n".join(re.sub(r"#.*$", "", line) for line in text.splitlines())
    base = PurePosixPath(meson_rel).parent
    library_is_shared = not re.search(r"default_library\s*=\s*static", text)
    variables: dict[str, list[str]] = {}
    libraries: list[NativeLibrary] = []
    events: list[tuple[int, str, re.Match[str]]] = [
        (m.start(), "assign", m) for m in _MESON_ASSIGN_RE.finditer(text)
    ] + [(m.start(), "library", m) for m in _MESON_LIBRARY_RE.finditer(text)]
    for _, kind, match in sorted(events, key=lambda event: event[0]):
        if kind == "assign":
            end = _meson_expression_end(text, match.end())
            found = _meson_values(text[match.end() : end], variables)
            if match.group(2) == "+=":
                variables.setdefault(match.group(1), []).extend(found)
            else:
                variables[match.group(1)] = found
            continue
        if match.group(1) == "library" and not library_is_shared:
            continue
        end = _meson_balanced(text, match.end() - 1)
        args = _meson_split_args(text[match.end() : end - 1])
        if not args:
            continue
        name_match = _MESON_STRING_RE.fullmatch(args[0])
        if name_match is None:
            continue
        items: list[str] = []
        for arg in args[1:]:
            key, colon, value = arg.partition(":")
            if colon and re.fullmatch(r"\s*[A-Za-z_]\w*\s*", key):
                if key.strip() == "sources":
                    items.extend(_meson_values(value, variables))
                continue
            items.extend(_meson_values(arg, variables))
        sources = _existing_sources(repo_root, base, items)
        if sources:
            name = name_match.group(1).replace("-", "_")
            libraries.append(NativeLibrary(meson_rel, "meson", name, sources))
    return libraries


def _meson_expression_end(text: str, start: int) -> int:
    """End of the right-hand side of an assignment starting at *start*."""
    index = start
    while index < len(text):
        char = text[index]
        if char in "([{":
            index = _meson_balanced(text, index)
            continue
        if char == "'":
            match = _MESON_STRING_RE.match(text, index)
            index = match.end() if match else index + 1
            continue
        if char == "\n":
            return index
        index += 1
    return index


# ---------------------------------------------------------------------------
# Cargo build scripts
# ---------------------------------------------------------------------------

_CC_CHAIN_RE = re.compile(r"\b(?P<kind>(?:cc::)?Build::new\(\)|cxx_build::bridges?\()")
_CC_COMPILE_RE = re.compile(r"\.compile\(\s*\"(?P<name>[^\"]+)\"\s*\)")
_CC_FILE_RE = re.compile(r"\.file\(\s*\"(?P<path>[^\"]+)\"\s*\)")
_CC_FILES_RE = re.compile(r"\.files\(\s*&?\s*(?:vec!)?\s*\[(?P<items>[^\]]*)\]")
_RUST_STRING_RE = re.compile(r"\"([^\"]*)\"")


def _cc_build_libraries(repo_root: Path, build_rel: str) -> list[NativeLibrary]:
    """``build.rs`` compiling C / C++ into the crate beside it: each
    ``cc::Build::new()`` or ``cxx_build::bridge(...)`` chain up to its
    ``.compile("name")``, with the paths given to ``.file`` / ``.files``."""
    base = PurePosixPath(build_rel).parent
    cargo_rel = "Cargo.toml" if str(base) == "." else f"{base}/Cargo.toml"
    if not (repo_root / cargo_rel).is_file():
        return []
    try:
        text = (repo_root / build_rel).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    text = re.sub(r"//[^\n]*", "", text)
    libraries: list[NativeLibrary] = []
    for start in _CC_CHAIN_RE.finditer(text):
        compile_match = _CC_COMPILE_RE.search(text, start.end())
        if compile_match is None:
            continue
        chain = text[start.start() : compile_match.end()]
        if _CC_CHAIN_RE.search(chain, len(start.group(0))):
            continue  # another chain starts before this one compiles
        items = [m.group("path") for m in _CC_FILE_RE.finditer(chain)]
        for files in _CC_FILES_RE.finditer(chain):
            items.extend(_RUST_STRING_RE.findall(files.group("items")))
        sources = _existing_sources(repo_root, base, items)
        if not sources:
            continue
        kind = "cxx" if start.group("kind").startswith("cxx_build") else "cc"
        libraries.append(NativeLibrary(build_rel, kind, compile_match.group("name"), sources))
    return libraries


# ---------------------------------------------------------------------------
# node-gyp
# ---------------------------------------------------------------------------


def _gyp_libraries(repo_root: Path, gyp_rel: str) -> list[NativeLibrary]:
    """``binding.gyp``: a Python literal with ``#`` comments."""
    try:
        text = (repo_root / gyp_rel).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    stripped = "\n".join(line for line in text.splitlines() if not line.lstrip().startswith("#"))
    try:
        data = ast.literal_eval(stripped)
    except (ValueError, SyntaxError, MemoryError, RecursionError):
        return []
    targets = data.get("targets") if isinstance(data, dict) else None
    base = PurePosixPath(gyp_rel).parent
    libraries: list[NativeLibrary] = []
    for target in targets if isinstance(targets, list) else []:
        if not isinstance(target, dict):
            continue
        name = target.get("target_name")
        sources = target.get("sources")
        if target.get("type", "loadable_module") not in ("loadable_module", "shared_library"):
            continue
        if not isinstance(name, str) or not isinstance(sources, list):
            continue
        items = [source for source in sources if isinstance(source, str)]
        found = _existing_sources(repo_root, base, items)
        if not found:
            continue
        outputs = [
            rel
            for config in ("Release", "Debug")
            if (rel := _resolve_rel(base, f"build/{config}/{name}.node")) is not None
        ]
        libraries.append(NativeLibrary(gyp_rel, "node-gyp", name, found, outputs))
    return libraries


# ---------------------------------------------------------------------------
# Compiler command lines (Makefile recipes, justfiles, package.json scripts)
# ---------------------------------------------------------------------------

_COMPILER_RE = re.compile(
    r"^(?:[\w.-]+-)?(?:gcc|g\+\+|clang|clang\+\+|cc|c\+\+)(?:-\d+(?:\.\d+)*)?$"
)
_SHARED_FLAGS = {"-shared", "-dynamiclib", "-bundle"}
_VALUED_FLAGS = {
    "-o",
    "-I",
    "-L",
    "-D",
    "-U",
    "-include",
    "-isystem",
    "-MF",
    "-MT",
    "-MQ",
    "-x",
    "-arch",
    "-framework",
    "-install_name",
    "-target",
}
_MAKE_VAR_RE = re.compile(r"\$[({]([A-Za-z_][A-Za-z0-9_]*)[)}]")
_MAKE_ASSIGN_RE = re.compile(r"^([A-Za-z_][A-Za-z0-9_]*)\s*(\?=|:=|::=|\+=|=)\s*(.*)$")
_MAKE_RULE_RE = re.compile(r"^([^\s:=#][^:=]*?)\s*::?(?!=)\s*(.*)$")
_MAKE_DEFAULTS = {"CC": "cc", "CXX": "c++"}


def _command_library(
    repo_root: Path, command_file: str, cwd: PurePosixPath, segment: str
) -> NativeLibrary | None:
    tokens = [token for token in _split_command(segment) if token]
    while tokens and ("=" in tokens[0] and not tokens[0].startswith("-")):
        tokens = tokens[1:]  # environment assignments
    if not tokens:
        return None
    compiler = tokens[0].lstrip("@-")
    if compiler.rsplit("/", 1)[-1] in ("emcc", "em++"):
        return _emscripten_library(repo_root, command_file, cwd, tokens[1:])
    if not _COMPILER_RE.match(compiler.rsplit("/", 1)[-1]):
        return None
    args = tokens[1:]
    if not _SHARED_FLAGS.intersection(args):
        return None
    options, positional = _command_options(args, _VALUED_FLAGS)
    output = options.get("-o")
    if not output or not _SHARED_OUTPUT_RE.search(output):
        return None
    items = [_object_source(repo_root, cwd, item) or item for item in positional]
    sources = _existing_sources(repo_root, cwd, items)
    if not sources:
        return None
    return NativeLibrary(command_file, "make", library_stem(output), sources)


_EMSCRIPTEN_GLUE_SUFFIXES = (".js", ".mjs", ".cjs", ".html")


def _emscripten_library(
    repo_root: Path, command_file: str, cwd: PurePosixPath, args: list[str]
) -> NativeLibrary | None:
    settings: dict[str, str] = {}
    rest: list[str] = []
    index = 0
    while index < len(args):
        token = args[index]
        if token == "-s" and index + 1 < len(args):
            setting = args[index + 1]
            index += 1
        elif token.startswith("-s") and "=" in token:
            setting = token[2:]
        else:
            rest.append(token)
            index += 1
            continue
        key, _, value = setting.partition("=")
        settings[key] = value
        index += 1
    options, positional = _command_options(rest, _VALUED_FLAGS)
    output = options.get("-o")
    if not output:
        return None
    output_rel = _resolve_rel(cwd, output)
    if output_rel is None:
        return None
    stem, dot, suffix = output_rel.rpartition(".")
    if not dot or f".{suffix}" not in (*_EMSCRIPTEN_GLUE_SUFFIXES, ".wasm"):
        return None
    outputs = [output_rel]
    if f".{suffix}" == ".html":
        outputs.append(f"{stem}.js")
    if f".{suffix}" != ".wasm":
        outputs.append(f"{stem}.wasm")
    items = [_object_source(repo_root, cwd, item) or item for item in positional]
    sources = _existing_sources(repo_root, cwd, items)
    if not sources:
        return None
    exported = settings.get("EXPORTED_FUNCTIONS")
    wasm_exports = None
    if exported is not None and not exported.startswith("@"):
        names = re.findall(r"[A-Za-z_$][\w$]*", exported)
        wasm_exports = [name.removeprefix("_") for name in names]
    name = stem.rsplit("/", 1)[-1]
    return NativeLibrary(
        command_file, "emscripten", name, sources, outputs, wasm_exports=wasm_exports
    )


def _object_source(repo_root: Path, cwd: PurePosixPath, item: str) -> str | None:
    """The source file an object file ``foo.o`` is compiled from, if unique."""
    if not item.endswith(".o"):
        return None
    stem = item[: -len(".o")]
    found = [
        f"{stem}{suffix}"
        for suffix in C_SOURCE_SUFFIXES
        if (rel := _resolve_rel(cwd, f"{stem}{suffix}")) is not None and (repo_root / rel).is_file()
    ]
    return found[0] if len(found) == 1 else None


def _make_recipe_commands(text: str) -> list[str]:
    """Recipe lines with automatic and simple variables expanded."""
    variables: dict[str, str] = {}
    commands: list[str] = []
    rule: tuple[list[str], list[str]] | None = None

    def expand(value: str, depth: int = 0) -> str:
        if depth > 8:
            return value
        expanded = _MAKE_VAR_RE.sub(
            lambda m: variables.get(m.group(1), _MAKE_DEFAULTS.get(m.group(1), m.group(0))),
            value,
        )
        return expanded if expanded == value else expand(expanded, depth + 1)

    for line in text.replace("\\\n", " ").splitlines():
        if line.startswith("\t"):
            if rule is None:
                continue
            targets, prereqs = rule
            command = line.strip()
            command = command.replace("$@", targets[0] if targets else "")
            command = command.replace("$^", " ".join(prereqs)).replace("$+", " ".join(prereqs))
            command = command.replace("$<", prereqs[0] if prereqs else "")
            commands.append(expand(command))
            continue
        stripped = line.split("#", 1)[0].rstrip()
        if not stripped.strip():
            continue
        assign = _MAKE_ASSIGN_RE.match(stripped.strip())
        if assign is not None:
            name, op, value = assign.groups()
            if op == "+=":
                variables[name] = f"{variables.get(name, '')} {value}".strip()
            elif op != "?=" or name not in variables:
                variables[name] = value
            rule = None
            continue
        rule_match = _MAKE_RULE_RE.match(stripped)
        if rule_match is not None:
            targets = expand(rule_match.group(1)).split()
            prereqs = expand(rule_match.group(2).split(";", 1)[0].split("|", 1)[0]).split()
            rule = (targets, prereqs)
        else:
            rule = None
    return commands


def _command_libraries(
    repo_root: Path, command_file: str, commands: Iterator[str] | list[str]
) -> list[NativeLibrary]:
    cwd = PurePosixPath(command_file).parent
    libraries: list[NativeLibrary] = []
    for command in commands:
        for segment in re.split(r"&&|\|\||;|\|", command):
            library = _command_library(repo_root, command_file, cwd, segment)
            if library is not None:
                libraries.append(library)
    return libraries


def discover_native_libraries(
    repo_root: Path,
    *,
    cmake_lists: list[str],
    meson_builds: list[str],
    makefiles: list[str],
    command_sources: Iterator[tuple[str, list[str]]],
    binding_gyps: list[str],
    build_scripts: list[str],
) -> list[NativeLibrary]:
    """Shared libraries built from repository C / C++ / Objective-C sources.

    *command_sources* yields ``(file, command lines)`` for build files whose
    lines are plain shell commands (package.json scripts, justfiles).
    """
    libraries: list[NativeLibrary] = []
    for rel in cmake_lists:
        libraries.extend(_cmake_libraries(repo_root, rel))
    for rel in meson_builds:
        libraries.extend(_meson_libraries(repo_root, rel))
    for rel in binding_gyps:
        libraries.extend(_gyp_libraries(repo_root, rel))
    for rel in build_scripts:
        libraries.extend(_cc_build_libraries(repo_root, rel))
    for rel in makefiles:
        try:
            text = (repo_root / rel).read_text(encoding="utf-8", errors="replace")
        except OSError:
            continue
        libraries.extend(_command_libraries(repo_root, rel, _make_recipe_commands(text)))
    for rel, commands in command_sources:
        libraries.extend(_command_libraries(repo_root, rel, commands))

    merged: dict[tuple[str, str], NativeLibrary] = {}
    for library in libraries:
        key = (library.config_rel, library.lib_name)
        existing = merged.get(key)
        if existing is None:
            merged[key] = library
            continue
        existing.sources.extend(s for s in library.sources if s not in existing.sources)
    return list(merged.values())
