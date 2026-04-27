"""Cross-language bridge detection (subprocess, FFI, etc.)."""

from __future__ import annotations

from typing import Optional

from .types import BridgePattern, EdgeInfo

_BRIDGE_PATTERNS: dict[str, tuple[BridgePattern, ...]] = {
    "python": (
        BridgePattern("subprocess.run", "invokes_binary", "subprocess"),
        BridgePattern("subprocess.Popen", "invokes_binary", "subprocess"),
        BridgePattern("subprocess.call", "invokes_binary", "subprocess"),
        BridgePattern("subprocess.check_call", "invokes_binary", "subprocess"),
        BridgePattern("subprocess.check_output", "invokes_binary", "subprocess"),
        BridgePattern("os.system", "invokes_binary", "subprocess"),
        BridgePattern("os.popen", "invokes_binary", "subprocess"),
        BridgePattern("os.execv", "invokes_binary", "subprocess"),
        BridgePattern("os.execvp", "invokes_binary", "subprocess"),
        BridgePattern("os.execvpe", "invokes_binary", "subprocess"),
        BridgePattern("os.execve", "invokes_binary", "subprocess"),
        BridgePattern("os.execl", "invokes_binary", "subprocess"),
        BridgePattern("os.execlp", "invokes_binary", "subprocess"),
        BridgePattern("os.execlpe", "invokes_binary", "subprocess"),
        BridgePattern("os.execle", "invokes_binary", "subprocess"),
        BridgePattern("os.spawnv", "invokes_binary", "subprocess"),
        BridgePattern("os.spawnvp", "invokes_binary", "subprocess"),
        BridgePattern("ctypes.CDLL", "loads_shared_library", "ffi"),
        BridgePattern("ctypes.cdll.LoadLibrary", "loads_shared_library", "ffi"),
        BridgePattern("ctypes.WinDLL", "loads_shared_library", "ffi"),
        BridgePattern("ctypes.PyDLL", "loads_shared_library", "ffi"),
        BridgePattern("cffi.FFI().dlopen", "loads_shared_library", "ffi"),
        # File I/O — external filesystem coupling.
        # Note: pathlib method chains (Path("f").read_text()) resolve to a
        # callee like "Path(...).read_text" at the AST level and cannot be
        # matched by exact signature; only bare built-in forms are listed.
        BridgePattern("open", "opens_file", "file_io"),
        BridgePattern("io.open", "opens_file", "file_io"),
    ),
    "javascript": (
        BridgePattern("child_process.exec", "invokes_binary", "subprocess"),
        BridgePattern("child_process.execFile", "invokes_binary", "subprocess"),
        BridgePattern("child_process.execSync", "invokes_binary", "subprocess"),
        BridgePattern("child_process.execFileSync", "invokes_binary", "subprocess"),
        BridgePattern("child_process.spawn", "invokes_binary", "subprocess"),
        BridgePattern("child_process.spawnSync", "invokes_binary", "subprocess"),
        BridgePattern("child_process.fork", "invokes_binary", "subprocess"),
        # File I/O
        BridgePattern("fs.readFile", "reads_file", "file_io"),
        BridgePattern("fs.readFileSync", "reads_file", "file_io"),
        BridgePattern("fs.writeFile", "writes_file", "file_io"),
        BridgePattern("fs.writeFileSync", "writes_file", "file_io"),
        BridgePattern("fs.promises.readFile", "reads_file", "file_io"),
        BridgePattern("fs.promises.writeFile", "writes_file", "file_io"),
    ),
    "java": (
        BridgePattern("Runtime.getRuntime().exec", "invokes_binary", "subprocess"),
        BridgePattern("Runtime.exec", "invokes_binary", "subprocess"),
        BridgePattern("System.loadLibrary", "loads_shared_library", "ffi"),
        BridgePattern("System.load", "loads_shared_library", "ffi"),
        BridgePattern("Runtime.getRuntime().loadLibrary", "loads_shared_library", "ffi"),
        BridgePattern("Runtime.getRuntime().load", "loads_shared_library", "ffi"),
        # File I/O
        BridgePattern("Files.readString", "reads_file", "file_io"),
        BridgePattern("Files.readAllBytes", "reads_file", "file_io"),
        BridgePattern("Files.writeString", "writes_file", "file_io"),
        BridgePattern("Files.write", "writes_file", "file_io"),
    ),
    "r": (
        BridgePattern("system", "invokes_binary", "subprocess"),
        BridgePattern("system2", "invokes_binary", "subprocess"),
        BridgePattern(".Call", "loads_native_module", "ffi"),
        BridgePattern(".External", "loads_native_module", "ffi"),
        BridgePattern("dyn.load", "loads_shared_library", "ffi"),
        BridgePattern("library.dynam", "loads_shared_library", "ffi"),
        # File I/O
        BridgePattern("readLines", "reads_file", "file_io"),
        BridgePattern("writeLines", "writes_file", "file_io"),
        BridgePattern("read.csv", "reads_file", "file_io"),
        BridgePattern("read.table", "reads_file", "file_io"),
        BridgePattern("write.csv", "writes_file", "file_io"),
    ),
}
# TypeScript/TSX share the JavaScript bridge patterns.
_BRIDGE_PATTERNS["typescript"] = _BRIDGE_PATTERNS["javascript"]
_BRIDGE_PATTERNS["tsx"] = _BRIDGE_PATTERNS["javascript"]

_BRIDGE_ARG_LIST_TYPES: frozenset[str] = frozenset({"argument_list", "arguments"})
_BRIDGE_STRING_NODE_TYPES: frozenset[str] = frozenset(
    {"string", "string_literal", "raw_string_literal", "interpreted_string_literal"}
)
_BRIDGE_LIST_NODE_TYPES: frozenset[str] = frozenset({"list", "tuple", "array"})
_BRIDGE_STRING_CONTENT_TYPES: frozenset[str] = frozenset({"string_content", "string_fragment"})


def _decode_bridge_string_node(node) -> str:
    """Return the unquoted content of a tree-sitter string-literal node.

    Works across grammars: Python uses ``string_content``, JS/TS uses
    ``string_fragment``, Java/R/etc embed the text directly. Falls back to
    the full node text with surrounding quotes/backticks stripped.
    """
    for sub in node.children:
        if sub.type in _BRIDGE_STRING_CONTENT_TYPES:
            return sub.text.decode("utf-8", errors="replace")
    return node.text.decode("utf-8", errors="replace").strip("'\"`")


def bridge_callee_signature(call_node, language: str) -> Optional[str]:
    """Return the canonical dotted-name of the callee for bridge matching.

    For most languages, the first child of the call node is the full callee
    expression (e.g. ``subprocess.run``, ``child_process.exec``,
    ``system``). Java's ``method_invocation`` interleaves ``object``,
    ``.``, and ``name`` as separate children, so we accumulate text up to
    the argument list to recover ``Runtime.getRuntime().exec``-style
    chains.
    """
    if not call_node.children:
        return None

    if language == "java":
        parts: list[str] = []
        for child in call_node.children:
            if child.type in _BRIDGE_ARG_LIST_TYPES:
                break
            parts.append(child.text.decode("utf-8", errors="replace"))
        sig = "".join(parts).strip()
        return sig or None

    return call_node.children[0].text.decode("utf-8", errors="replace").strip()


def bridge_first_string_arg(call_node, language: str) -> tuple[Optional[str], bool]:
    """Return ``(string_value, is_literal)`` for the first call argument.

    Recognizes direct string literals and list/array/tuple first elements.
    For R, arguments are wrapped in an extra ``argument`` node; for Java
    the first arg may be a ``string_literal``-like node containing the
    text directly. Returns ``(None, False)`` when the first real argument
    is not a literal.
    """
    del language  # current dispatch is uniform across grammars
    arg_list = next(
        (c for c in call_node.children if c.type in _BRIDGE_ARG_LIST_TYPES),
        None,
    )
    if arg_list is None:
        return None, False

    for arg in arg_list.children:
        if arg.type in (",", "(", ")", "{", "}", "[", "]"):
            continue
        if arg.type == "argument" and arg.children:
            # R wraps each call argument in an `argument` node.
            arg = next(
                (c for c in arg.children if c.type not in (",", "(", ")")),
                arg.children[0],
            )
        if arg.type in _BRIDGE_STRING_NODE_TYPES:
            return _decode_bridge_string_node(arg), True
        if arg.type in _BRIDGE_LIST_NODE_TYPES:
            for item in arg.children:
                if item.type in _BRIDGE_STRING_NODE_TYPES:
                    return _decode_bridge_string_node(item), True
            return None, False
        return None, False
    return None, False


def detect_cross_language_bridge(
    call_node,
    language: str,
    file_path: str,
    caller: str,
) -> list[EdgeInfo]:
    """Emit ``CROSS_ARTIFACT`` edges based on the per-language bridge registry.

    Language-agnostic: the dispatcher looks up patterns by language, then
    delegates signature extraction and string-arg parsing to thin adapters.
    Adding a new language only requires (1) entries in ``_BRIDGE_PATTERNS``
    and (2) — if its tree-sitter call-node shape differs from the default
    — a branch in ``bridge_callee_signature``.
    """
    patterns = _BRIDGE_PATTERNS.get(language)
    if not patterns:
        return []

    sig = bridge_callee_signature(call_node, language)
    if not sig:
        return []

    matched = next((p for p in patterns if p.call_signature == sig), None)
    if matched is None:
        return []

    line_no = call_node.start_point[0] + 1
    literal, is_literal = bridge_first_string_arg(call_node, language)
    if is_literal and literal:
        target = literal
        confidence: float = 0.8
        tier = "HIGH"
    else:
        target = f"<dynamic:{sig}@{file_path}:{line_no}>"
        confidence = 0.2
        tier = "LOW"

    return [
        EdgeInfo(
            kind="CROSS_ARTIFACT",
            source=caller,
            target=target,
            file_path=file_path,
            line=line_no,
            extra={
                "relationship_role": matched.relationship_role,
                "bridge_kind": matched.bridge_kind,
                "evidence_kind": "syntax",
                "evidence_source": sig,
                "source_language": language,
                "target_language": "unknown",
                "confidence": confidence,
                "confidence_tier": tier,
            },
        )
    ]
