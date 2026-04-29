"""Cross-language bridge detection (subprocess, FFI, etc.)."""

from __future__ import annotations

from typing import Optional

from ._base.types import BridgePattern, EdgeInfo

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
    "go": (
        BridgePattern("exec.Command", "invokes_binary", "subprocess"),
        BridgePattern("exec.CommandContext", "invokes_binary", "subprocess"),
        BridgePattern("syscall.Exec", "invokes_binary", "subprocess"),
        BridgePattern("os.StartProcess", "invokes_binary", "subprocess"),
        BridgePattern("os.Open", "reads_file", "file_io"),
        BridgePattern("os.Create", "writes_file", "file_io"),
        BridgePattern("os.ReadFile", "reads_file", "file_io"),
        BridgePattern("os.WriteFile", "writes_file", "file_io"),
        BridgePattern("ioutil.ReadFile", "reads_file", "file_io"),
        BridgePattern("ioutil.WriteFile", "writes_file", "file_io"),
        BridgePattern("plugin.Open", "loads_shared_library", "ffi"),
        BridgePattern("syscall.LoadLibrary", "loads_shared_library", "ffi"),
        BridgePattern("syscall.NewLazyDLL", "loads_shared_library", "ffi"),
    ),
    "rust": (
        BridgePattern("std::process::Command::new", "invokes_binary", "subprocess"),
        BridgePattern("Command::new", "invokes_binary", "subprocess"),
        BridgePattern("std::fs::read", "reads_file", "file_io"),
        BridgePattern("std::fs::read_to_string", "reads_file", "file_io"),
        BridgePattern("std::fs::write", "writes_file", "file_io"),
        BridgePattern("std::fs::File::open", "reads_file", "file_io"),
        BridgePattern("std::fs::File::create", "writes_file", "file_io"),
        BridgePattern("fs::read", "reads_file", "file_io"),
        BridgePattern("fs::read_to_string", "reads_file", "file_io"),
        BridgePattern("fs::write", "writes_file", "file_io"),
        BridgePattern("File::open", "reads_file", "file_io"),
        BridgePattern("File::create", "writes_file", "file_io"),
        BridgePattern("libloading::Library::new", "loads_shared_library", "ffi"),
        BridgePattern("Library::new", "loads_shared_library", "ffi"),
    ),
    "ruby": (
        # Bare subprocess calls (no explicit receiver — tree-sitter emits
        # method as first child; accumulation loop still works).
        BridgePattern("system", "invokes_binary", "subprocess"),
        BridgePattern("exec", "invokes_binary", "subprocess"),
        BridgePattern("spawn", "invokes_binary", "subprocess"),
        # Module-qualified calls (receiver.method — needs accumulation branch).
        BridgePattern("Kernel.system", "invokes_binary", "subprocess"),
        BridgePattern("Process.spawn", "invokes_binary", "subprocess"),
        BridgePattern("IO.popen", "invokes_binary", "subprocess"),
        BridgePattern("Open3.capture3", "invokes_binary", "subprocess"),
        BridgePattern("Open3.popen3", "invokes_binary", "subprocess"),
        BridgePattern("File.read", "reads_file", "file_io"),
        BridgePattern("File.write", "writes_file", "file_io"),
        BridgePattern("File.open", "opens_file", "file_io"),
        BridgePattern("File.readlines", "reads_file", "file_io"),
        BridgePattern("IO.read", "reads_file", "file_io"),
        BridgePattern("IO.write", "writes_file", "file_io"),
        BridgePattern("Fiddle.dlopen", "loads_shared_library", "ffi"),
    ),
    "swift": (
        BridgePattern("Process.run", "invokes_binary", "subprocess"),
        BridgePattern("String.contentsOf", "reads_file", "file_io"),
        BridgePattern("Data.contentsOf", "reads_file", "file_io"),
        BridgePattern("FileManager.contentsOfFile", "reads_file", "file_io"),
        BridgePattern("FileManager.createFile", "writes_file", "file_io"),
        BridgePattern("dlopen", "loads_shared_library", "ffi"),
        BridgePattern("Bundle.load", "loads_shared_library", "ffi"),
    ),
    "c": (
        BridgePattern("system", "invokes_binary", "subprocess"),
        BridgePattern("popen", "invokes_binary", "subprocess"),
        BridgePattern("execvp", "invokes_binary", "subprocess"),
        BridgePattern("execv", "invokes_binary", "subprocess"),
        BridgePattern("execl", "invokes_binary", "subprocess"),
        BridgePattern("posix_spawn", "invokes_binary", "subprocess"),
        BridgePattern("fopen", "opens_file", "file_io"),
        BridgePattern("open", "opens_file", "file_io"),
        BridgePattern("fread", "reads_file", "file_io"),
        BridgePattern("fwrite", "writes_file", "file_io"),
        BridgePattern("dlopen", "loads_shared_library", "ffi"),
        BridgePattern("LoadLibrary", "loads_shared_library", "ffi"),
    ),
    "csharp": (
        BridgePattern("Process.Start", "invokes_binary", "subprocess"),
        BridgePattern("System.Diagnostics.Process.Start", "invokes_binary", "subprocess"),
        BridgePattern("File.ReadAllText", "reads_file", "file_io"),
        BridgePattern("File.ReadAllBytes", "reads_file", "file_io"),
        BridgePattern("File.ReadAllLines", "reads_file", "file_io"),
        BridgePattern("File.WriteAllText", "writes_file", "file_io"),
        BridgePattern("File.WriteAllBytes", "writes_file", "file_io"),
        BridgePattern("File.OpenRead", "reads_file", "file_io"),
        BridgePattern("File.OpenWrite", "writes_file", "file_io"),
        BridgePattern("File.Create", "writes_file", "file_io"),
        BridgePattern("Assembly.LoadFile", "loads_shared_library", "ffi"),
        BridgePattern("NativeLibrary.Load", "loads_shared_library", "ffi"),
    ),
    "kotlin": (
        BridgePattern("Runtime.getRuntime().exec", "invokes_binary", "subprocess"),
        BridgePattern("ProcessBuilder.start", "invokes_binary", "subprocess"),
        BridgePattern("System.loadLibrary", "loads_shared_library", "ffi"),
        BridgePattern("System.load", "loads_shared_library", "ffi"),
        BridgePattern("Files.readString", "reads_file", "file_io"),
        BridgePattern("Files.readAllBytes", "reads_file", "file_io"),
        BridgePattern("Files.writeString", "writes_file", "file_io"),
        BridgePattern("Files.write", "writes_file", "file_io"),
        BridgePattern("File.readText", "reads_file", "file_io"),
        BridgePattern("File.writeText", "writes_file", "file_io"),
        BridgePattern("File.readLines", "reads_file", "file_io"),
        BridgePattern("File.bufferedReader", "reads_file", "file_io"),
    ),
    "php": (
        # Bare subprocess calls (function_call_expression — default extraction works).
        BridgePattern("exec", "invokes_binary", "subprocess"),
        BridgePattern("shell_exec", "invokes_binary", "subprocess"),
        BridgePattern("system", "invokes_binary", "subprocess"),
        BridgePattern("passthru", "invokes_binary", "subprocess"),
        BridgePattern("proc_open", "invokes_binary", "subprocess"),
        BridgePattern("popen", "invokes_binary", "subprocess"),
        BridgePattern("file_get_contents", "reads_file", "file_io"),
        BridgePattern("file_put_contents", "writes_file", "file_io"),
        BridgePattern("fopen", "opens_file", "file_io"),
        BridgePattern("fread", "reads_file", "file_io"),
        BridgePattern("fwrite", "writes_file", "file_io"),
        BridgePattern("readfile", "reads_file", "file_io"),
        # Static-method FFI calls (scoped_call_expression — needs accumulation branch).
        BridgePattern("FFI::cdef", "loads_shared_library", "ffi"),
        BridgePattern("FFI::load", "loads_shared_library", "ffi"),
    ),
    "perl": (
        BridgePattern("system", "invokes_binary", "subprocess"),
        BridgePattern("exec", "invokes_binary", "subprocess"),
        BridgePattern("open", "opens_file", "file_io"),
        BridgePattern("File::Slurp::read_file", "reads_file", "file_io"),
        BridgePattern("File::Slurp::write_file", "writes_file", "file_io"),
        BridgePattern("DynaLoader::dl_load_file", "loads_shared_library", "ffi"),
    ),
    "scala": (
        BridgePattern("Runtime.getRuntime().exec", "invokes_binary", "subprocess"),
        BridgePattern("scala.sys.process.Process", "invokes_binary", "subprocess"),
        BridgePattern("System.loadLibrary", "loads_shared_library", "ffi"),
        BridgePattern("System.load", "loads_shared_library", "ffi"),
        BridgePattern("Files.readString", "reads_file", "file_io"),
        BridgePattern("Files.readAllBytes", "reads_file", "file_io"),
        BridgePattern("Files.writeString", "writes_file", "file_io"),
        BridgePattern("Files.write", "writes_file", "file_io"),
        BridgePattern("scala.io.Source.fromFile", "reads_file", "file_io"),
    ),
    "lua": (
        BridgePattern("os.execute", "invokes_binary", "subprocess"),
        BridgePattern("io.popen", "invokes_binary", "subprocess"),
        BridgePattern("io.open", "opens_file", "file_io"),
        BridgePattern("io.lines", "reads_file", "file_io"),
        BridgePattern("io.read", "reads_file", "file_io"),
        BridgePattern("io.write", "writes_file", "file_io"),
        BridgePattern("package.loadlib", "loads_shared_library", "ffi"),
        BridgePattern("loadlib", "loads_shared_library", "ffi"),
    ),
    "julia": (
        BridgePattern("run", "invokes_binary", "subprocess"),
        BridgePattern("readchomp", "invokes_binary", "subprocess"),
        BridgePattern("open", "opens_file", "file_io"),
        BridgePattern("read", "reads_file", "file_io"),
        BridgePattern("write", "writes_file", "file_io"),
        BridgePattern("readlines", "reads_file", "file_io"),
        BridgePattern("Libdl.dlopen", "loads_shared_library", "ffi"),
        BridgePattern("dlopen", "loads_shared_library", "ffi"),
        BridgePattern("ccall", "loads_shared_library", "ffi"),
    ),
}
# TypeScript/TSX share the JavaScript bridge patterns.
_BRIDGE_PATTERNS["typescript"] = _BRIDGE_PATTERNS["javascript"]
_BRIDGE_PATTERNS["tsx"] = _BRIDGE_PATTERNS["javascript"]
# C++ extends C with std:: equivalents and stream constructors.
_BRIDGE_PATTERNS["cpp"] = _BRIDGE_PATTERNS["c"] + (
    BridgePattern("std::system", "invokes_binary", "subprocess"),
    BridgePattern("std::ifstream", "opens_file", "file_io"),
    BridgePattern("std::ofstream", "opens_file", "file_io"),
    BridgePattern("std::fstream", "opens_file", "file_io"),
    BridgePattern("boost::process::child", "invokes_binary", "subprocess"),
)
# Objective-C: C-style calls are covered by c patterns; bracket-send
# method patterns (NSTask, NSString) require a different extraction path
# (message_expression node shape differs fundamentally). Deferred.
_BRIDGE_PATTERNS["objc"] = _BRIDGE_PATTERNS["c"]
# Lua and Luau share patterns.
_BRIDGE_PATTERNS["luau"] = _BRIDGE_PATTERNS["lua"]

_BRIDGE_ARG_LIST_TYPES: frozenset[str] = frozenset(
    {"argument_list", "arguments", "value_arguments"}
)
_BRIDGE_STRING_NODE_TYPES: frozenset[str] = frozenset(
    {
        "string",
        "string_literal",
        "raw_string_literal",
        "interpreted_string_literal",
        "line_string_literal",  # Swift
        "encapsed_string",  # PHP double-quoted strings
        "interpolated_string_literal",  # Perl double-quoted strings
    }
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

    # Java, Ruby, and PHP (scoped/member calls) interleave receiver, separator,
    # and method name as sibling children rather than a single callee node.
    # Accumulate all text before the argument list to reconstruct the full
    # dotted/scoped signature (e.g. "File.read", "FFI::cdef").
    if language in {"java", "ruby", "php"}:
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
    For R, arguments are wrapped in an extra ``argument`` node; Swift/Kotlin
    wrap args in ``value_argument`` inside ``call_suffix`` → ``value_arguments``.
    Perl has no argument_list node — string args are direct children of the
    call node. Returns ``(None, False)`` when the first real argument is not
    a literal.
    """
    # Swift/Kotlin: arguments live inside call_suffix → value_arguments.
    node_to_search = call_node
    for child in call_node.children:
        if child.type == "call_suffix":
            node_to_search = child
            break

    arg_list = next(
        (c for c in node_to_search.children if c.type in _BRIDGE_ARG_LIST_TYPES),
        None,
    )

    # Perl: function_call_expression has no argument_list; string args are
    # direct children after the function name and opening parenthesis.
    if arg_list is None and language == "perl":
        skip_types = frozenset({",", "(", ")", "{", "}", "[", "]", "function"})
        for child in call_node.children:
            if child.type in skip_types:
                continue
            if child.type in _BRIDGE_STRING_NODE_TYPES:
                return _decode_bridge_string_node(child), True
            return None, False
        return None, False

    if arg_list is None:
        return None, False

    for arg in arg_list.children:
        if arg.type in (",", "(", ")", "{", "}", "[", "]"):
            continue
        if arg.type in {"argument", "value_argument"} and arg.children:
            # R wraps each call argument in an `argument` node;
            # Swift/Kotlin wrap in `value_argument`.
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
