"""Tree-sitter based multi-language code parser.

Extracts structural nodes (classes, functions, imports, types) and edges
(calls, inheritance, contains) from source files.
"""

from __future__ import annotations

import logging
import re
from pathlib import Path
from typing import Optional

from ..tsconfig_resolver import TsconfigResolver
from . import bridges, dispatch, grammars
from .bridges import _BRIDGE_PATTERNS
from .languages import SPECIAL_HANDLERS as _SPECIAL_HANDLERS
from .languages import dart as _dart_lang
from .languages import markdown as _markdown_lang
from .languages import notebook as _notebook_lang
from .languages import rescript as _rescript_lang
from .languages import svelte as _svelte_lang
from .languages import vue as _vue_lang
from .test_detection import (
    _TEST_RUNNER_NAMES,
)
from .test_detection import (
    is_test_file as _is_test_file,
)
from .test_detection import (
    is_test_function as _is_test_function,
)
from .types import EdgeInfo, NodeInfo

_SQL_TABLE_RE = re.compile(
    r"(?:FROM|JOIN|INTO|CREATE\s+(?:OR\s+REPLACE\s+)?(?:TABLE|VIEW)|INSERT\s+OVERWRITE)"
    r"\s+((?:`[^`]+`|\w+)(?:\.(?:`[^`]+`|\w+))*)",
    re.IGNORECASE,
)

logger = logging.getLogger(__name__)

_TERRAFORM_REFERENCE_RE = re.compile(
    r"\b(?:(data)\.([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*)"
    r"|((?:module|var|local|output|provider|check))\.([A-Za-z_][A-Za-z0-9_]*)"
    r"|([A-Za-z_][A-Za-z0-9_]*)\.([A-Za-z_][A-Za-z0-9_]*))\b"
)
_TERRAFORM_CALL_RE = re.compile(r"\b([A-Za-z_][A-Za-z0-9_]*)\s*\(")
_TERRAFORM_REFERENCE_SKIP_ROOTS = frozenset(
    {
        "count",
        "each",
        "ingress",
        "egress",
        "path",
        "self",
        "terraform",
    }
)
_TERRAFORM_CALL_SKIP = frozenset({"for", "if"})


# Tree-sitter node type mappings per language
# Maps (language) -> dict of semantic role -> list of TS node types
_CLASS_TYPES: dict[str, list[str]] = {
    "python": ["class_definition"],
    "javascript": ["class_declaration", "class"],
    "typescript": ["class_declaration", "class", "interface_declaration"],
    "tsx": ["class_declaration", "class", "interface_declaration"],
    "go": ["type_declaration"],
    "rust": ["struct_item", "enum_item", "impl_item"],
    "java": ["class_declaration", "interface_declaration", "enum_declaration"],
    "c": ["struct_specifier", "type_definition"],
    "cpp": ["class_specifier", "struct_specifier"],
    "csharp": [
        "class_declaration",
        "interface_declaration",
        "enum_declaration",
        "struct_declaration",
    ],
    "ruby": ["class", "module"],
    "r": [],  # Classes detected via call pattern-matching, not AST node types
    "perl": ["package_statement", "class_statement", "role_statement"],
    "kotlin": ["class_declaration", "object_declaration"],
    "swift": ["class_declaration", "struct_declaration", "protocol_declaration"],
    "php": ["class_declaration", "interface_declaration"],
    "scala": [
        "class_definition",
        "trait_definition",
        "object_definition",
        "enum_definition",
    ],
    "solidity": [
        "contract_declaration",
        "interface_declaration",
        "library_declaration",
        "struct_declaration",
        "enum_declaration",
        "error_declaration",
        "user_defined_type_definition",
    ],
    "dart": ["class_definition", "mixin_declaration", "enum_declaration"],
    "lua": [],  # Lua has no class keyword; table-based OOP handled via constructs handler
    "luau": ["type_definition"],  # Luau type aliases; table-based OOP via constructs handler
    "objc": [
        "class_interface",
        "class_implementation",
        "category_interface",
        "protocol_declaration",
    ],
    "bash": [],  # Shell has no classes
    # Elixir: `defmodule Name do ... end` is a ``call`` node whose first
    # identifier is literally "defmodule". Dispatched via
    # _extract_elixir_constructs to avoid matching every ``call`` here.
    "elixir": [],
    "zig": ["container_declaration"],
    "powershell": ["class_statement"],
    "julia": ["module_definition", "struct_definition", "abstract_definition"],
    "gdscript": ["class_definition", "class_name_statement"],
    "terraform": [],
}

_FUNCTION_TYPES: dict[str, list[str]] = {
    "python": ["function_definition"],
    "javascript": ["function_declaration", "method_definition", "arrow_function"],
    "typescript": ["function_declaration", "method_definition", "arrow_function"],
    "tsx": ["function_declaration", "method_definition", "arrow_function"],
    "go": ["function_declaration", "method_declaration"],
    "rust": ["function_item"],
    "java": ["method_declaration", "constructor_declaration"],
    "c": ["function_definition"],
    "cpp": ["function_definition"],
    "csharp": ["method_declaration", "constructor_declaration"],
    "ruby": ["method", "singleton_method"],
    "r": ["function_definition"],
    "perl": ["subroutine_declaration_statement", "method_declaration_statement"],
    "kotlin": ["function_declaration"],
    "swift": ["function_declaration"],
    "php": ["function_definition", "method_declaration"],
    "scala": ["function_definition", "function_declaration"],
    # Solidity: events and modifiers use kind="Function" because the graph
    # schema has no dedicated kind for them.  State variables are also modeled
    # as Function nodes (public ones auto-generate getters) and distinguished
    # via extra["solidity_kind"].
    "solidity": [
        "function_definition",
        "constructor_definition",
        "modifier_definition",
        "event_definition",
        "fallback_receive_definition",
    ],
    # Dart: function_signature covers both top-level functions and class methods
    # (class methods appear as method_signature > function_signature pairs;
    # the parser recurses into method_signature generically and then matches
    # function_signature inside it).
    "dart": ["function_signature"],
    "lua": ["function_declaration"],
    "luau": ["function_declaration"],
    # Objective-C: method_definition lives inside implementation_definition
    # inside class_implementation. C-style function_definition is also present
    # for main() and helper functions.
    "objc": ["method_definition", "function_definition"],
    # Bash: only function_definition; everything else is a command.
    "bash": ["function_definition"],
    # Elixir: def/defp/defmacro are all ``call`` nodes whose first
    # identifier matches. Dispatched via _extract_elixir_constructs.
    "elixir": [],
    "zig": ["fn_proto", "fn_decl"],
    "powershell": ["function_statement"],
    "julia": [
        "function_definition",
        "short_function_definition",
        "macro_definition",
    ],
    "gdscript": ["function_definition"],
    "terraform": [],
}

_IMPORT_TYPES: dict[str, list[str]] = {
    "python": ["import_statement", "import_from_statement"],
    "javascript": ["import_statement"],
    "typescript": ["import_statement"],
    "tsx": ["import_statement"],
    "go": ["import_declaration"],
    "rust": ["use_declaration"],
    "java": ["import_declaration"],
    "c": ["preproc_include"],
    "cpp": ["preproc_include"],
    "csharp": ["using_directive"],
    "ruby": ["call"],  # require/require_relative
    "r": ["call"],  # library(), require(), source() — filtered downstream
    "perl": ["use_statement", "require_expression"],
    "kotlin": ["import_header"],
    "swift": ["import_declaration"],
    "php": ["namespace_use_declaration"],
    "scala": ["import_declaration"],
    "solidity": ["import_directive"],
    # Dart: import_or_export wraps library_import > import_specification > configurable_uri
    "dart": ["import_or_export"],
    # Lua/Luau: require() is a function_call, handled via _extract_lua_constructs
    "lua": [],
    "luau": [],
    # Objective-C: #import "..." and #include "..." both arrive as preproc_include
    # (tree-sitter-objc doesn't distinguish via a separate preproc_import node).
    "objc": ["preproc_include"],
    # Bash: source / . <file> is a command — handled in _extract_bash_source below.
    "bash": [],
    # Elixir: alias/import/require/use are all ``call`` nodes —
    # handled in _extract_elixir_constructs.
    "elixir": [],
    # Zig: @import("...") is a builtin_call_expr — handled
    # generically via call types below.
    "zig": [],
    "powershell": [],
    # Julia: import/using are import_statement nodes.
    "julia": ["import_statement", "using_statement"],
    "gdscript": ["extends_statement"],
    "terraform": [],
}

_CALL_TYPES: dict[str, list[str]] = {
    "python": ["call"],
    "javascript": ["call_expression", "new_expression"],
    "typescript": ["call_expression", "new_expression"],
    "tsx": ["call_expression", "new_expression"],
    "go": ["call_expression"],
    "rust": ["call_expression", "macro_invocation"],
    "java": ["method_invocation", "object_creation_expression"],
    "c": ["call_expression"],
    "cpp": ["call_expression"],
    "csharp": ["invocation_expression", "object_creation_expression"],
    "ruby": ["call", "method_call"],
    "r": ["call"],
    "perl": [
        "function_call_expression",
        "method_call_expression",
        "ambiguous_function_call_expression",
    ],
    "kotlin": ["call_expression"],
    "swift": ["call_expression"],
    "php": [
        "function_call_expression",
        "member_call_expression",
        "scoped_call_expression",
        "nullsafe_member_call_expression",
    ],
    "scala": ["call_expression", "instance_expression", "generic_function"],
    "solidity": ["call_expression"],
    "lua": ["function_call"],
    "luau": ["function_call"],
    # Objective-C: [receiver message:args] produces message_expression;
    # C-style foo(x) produces call_expression.
    "objc": ["message_expression", "call_expression"],
    # Bash: every command invocation is a "command" node.
    "bash": ["command"],
    # Elixir: everything is a ``call`` node — dispatched via
    # _extract_elixir_constructs which filters out def/defmodule/alias/etc.
    # before treating what's left as a real call.
    "elixir": [],
    "zig": ["call_expression", "builtin_call_expr"],
    "powershell": ["command_expression"],
    "julia": ["call_expression", "macrocall_expression"],
    "gdscript": ["call", "attribute_call"],
    "terraform": [],
}

# ---------------------------------------------------------------------------
# Type-role resolution helpers (module-level, used by CodeParser._extract_classes)
# ---------------------------------------------------------------------------

_TS_NODE_TO_TYPE_ROLE: dict[str, str] = {
    "interface_declaration": "interface",
    "protocol_declaration": "protocol",
    "trait_definition": "trait",
    "trait_declaration": "trait",
    "mixin_declaration": "mixin",
    "abstract_definition": "abstract_type",  # Julia
    "enum_declaration": "enum",
    "enum_definition": "enum",
    "struct_declaration": "struct",
    "struct_specifier": "struct",
    "struct_definition": "struct",
    "object_definition": "class",  # Scala companion object
}

_CONTRACT_ROLES: frozenset[str] = frozenset({"interface", "protocol", "trait"})

_SWIFT_KIND_TO_ROLE: dict[str, str] = {
    "class": "class",
    "struct": "struct",
    "enum": "enum",
    "actor": "class",
    "extension": "class",
    "protocol": "protocol",
}


def _resolve_type_role(node, language: str) -> tuple[str, bool, bool]:
    """Return (type_role, is_abstract, is_contract) for a class-like AST node.

    type_role is one of: class, abstract_class, interface, protocol, trait,
    abstract_type, mixin, enum, struct.
    """
    role = _TS_NODE_TO_TYPE_ROLE.get(node.type, "class")

    if language == "swift":
        if node.type == "protocol_declaration":
            role = "protocol"
        elif node.type == "class_declaration":
            _swift_kws = {"class", "struct", "enum", "actor", "extension"}
            for kw_child in node.children:
                kw = kw_child.text.decode("utf-8", errors="replace")
                if kw in _swift_kws:
                    role = _SWIFT_KIND_TO_ROLE.get(kw, "class")
                    break

    if language in ("java", "csharp", "kotlin") and role == "class":
        for child in node.children:
            if child.type == "modifiers":
                mod_text = child.text.decode("utf-8", errors="replace")
                if "abstract" in mod_text.split():
                    role = "abstract_class"
                    break

    if language == "python" and role == "class":
        for child in node.children:
            if child.type == "argument_list":
                for arg in child.children:
                    if arg.type in ("identifier", "attribute"):
                        arg_name = arg.text.decode("utf-8", errors="replace")
                        if arg_name in ("ABC", "ABCMeta") or arg_name.endswith(".ABC"):
                            role = "abstract_class"
                            break

    # Dart: abstract modifier appears as a keyword sibling of the class keyword.
    if language == "dart" and role == "class":
        for child in node.children:
            if child.type == "abstract" or child.text == b"abstract":
                role = "abstract_class"
                break

    is_contract = role in _CONTRACT_ROLES
    is_abstract = is_contract or role in ("abstract_class", "abstract_type")
    return role, is_abstract, is_contract


# ---------------------------------------------------------------------------
# Parser
# ---------------------------------------------------------------------------


class CodeParser:
    """Parses source files using Tree-sitter and extracts structural information."""

    _MODULE_CACHE_MAX = 15_000  # Evict cache to cap memory on huge monorepos

    def __init__(self) -> None:
        self._parsers: dict[str, object] = {}
        self._module_file_cache: dict[str, Optional[str]] = {}
        self._export_symbol_cache: dict[str, Optional[str]] = {}
        self._tsconfig_resolver = TsconfigResolver()
        # Per-parse cache of Dart pubspec root lookups; see #87
        self._dart_pubspec_cache: dict[tuple[str, str], Optional[Path]] = {}

    def _get_parser(self, language: str):
        return grammars.get_parser(language, self._parsers)

    def detect_language(self, path: Path) -> Optional[str]:
        return dispatch.detect_language(path)

    def parse_file(self, path: Path) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        """Parse a single file and return extracted nodes and edges."""
        try:
            source = path.read_bytes()
        except (OSError, PermissionError):
            return [], []
        return self.parse_bytes(path, source)

    def parse_bytes(self, path: Path, source: bytes) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        """Parse pre-read bytes and return extracted nodes and edges.

        This avoids re-reading the file from disk, eliminating TOCTOU gaps
        when the caller has already read the bytes (e.g. for hashing).
        """
        language = self.detect_language(path)
        if not language:
            return [], []

        # Vue SFCs: parse with vue parser, then delegate script blocks to JS/TS
        if language == "vue":
            return self._parse_vue(path, source)

        # Svelte SFCs: same approach as Vue — extract <script> blocks
        if language == "svelte":
            return self._parse_svelte(path, source)

        # Jupyter notebooks: extract code cells and parse as Python
        if language == "notebook":
            return self._parse_notebook(path, source)

        # Markdown docs: sections + links + dependency directives.
        if language == "markdown":
            return self._parse_markdown(path, source)

        # Databricks .py notebook exports
        if language == "python" and (
            source.startswith(b"# Databricks notebook source\n")
            or source.startswith(b"# Databricks notebook source\r\n")
        ):
            return self._parse_databricks_py_notebook(path, source)

        # ReScript: regex-based parser (no tree-sitter grammar bundled).
        if language == "rescript":
            return self._parse_rescript(path, source)

        parser = self._get_parser(language)
        if not parser:
            return [], []

        tree = parser.parse(source)
        nodes: list[NodeInfo] = []
        edges: list[EdgeInfo] = []
        file_path_str = str(path)

        # File node
        test_file = _is_test_file(file_path_str)
        nodes.append(
            NodeInfo(
                kind="File",
                name=file_path_str,
                file_path=file_path_str,
                line_start=1,
                line_end=source.count(b"\n") + 1,
                language=language,
                is_test=test_file,
            )
        )

        # Pre-scan for import mappings and defined names
        import_map, defined_names = self._collect_file_scope(
            tree.root_node,
            language,
            source,
        )

        # Walk the tree
        self._extract_from_tree(
            tree.root_node,
            source,
            language,
            file_path_str,
            nodes,
            edges,
            import_map=import_map,
            defined_names=defined_names,
        )

        # Resolve bare call targets to qualified names using same-file definitions
        edges = self._resolve_call_targets(nodes, edges, file_path_str)

        # Generate TESTED_BY edges: when a test function calls a production
        # function, create an edge from the production function back to the test.
        if test_file:
            test_qnames = set()
            for n in nodes:
                if n.is_test:
                    qn = self._qualify(n.name, n.file_path, n.parent_name)
                    test_qnames.add(qn)
            for edge in list(edges):
                if edge.kind == "CALLS" and edge.source in test_qnames:
                    edges.append(
                        EdgeInfo(
                            kind="TESTED_BY",
                            source=edge.target,
                            target=edge.source,
                            file_path=edge.file_path,
                            line=edge.line,
                        )
                    )

        return nodes, edges

    def _parse_vue(
        self,
        path: Path,
        source: bytes,
    ) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        """Delegates to languages.vue."""
        return _vue_lang.parse(self, path, source)

    def _parse_svelte(
        self,
        path: Path,
        source: bytes,
    ) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        """Delegates to languages.svelte."""
        return _svelte_lang.parse(self, path, source)

    def _parse_markdown(
        self,
        path: Path,
        source: bytes,
    ) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        """Parse Markdown documents into section nodes and dependency edges."""
        return _markdown_lang.parse(self, path, source)

    def _parse_notebook(
        self,
        path: Path,
        source: bytes,
    ) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        """Parse a Jupyter notebook by extracting code cells."""
        return _notebook_lang.parse(self, path, source)

    def _parse_databricks_py_notebook(
        self,
        path: Path,
        source: bytes,
    ) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        """Parse a Databricks .py notebook export."""
        return _notebook_lang.parse_databricks_py(self, path, source)

    # ------------------------------------------------------------------
    # ReScript: regex-based structural parser (no tree-sitter grammar
    # is bundled for ReScript, so we extract best-effort structure via
    # comment-stripping + line-anchored regex + brace-counted module scan).
    # ------------------------------------------------------------------

    def _parse_rescript(
        self,
        path: Path,
        source: bytes,
    ) -> tuple[list[NodeInfo], list[EdgeInfo]]:
        """Parse a ReScript `.res` or `.resi` file.

        Extracts modules, let bindings, types, external bindings, open/include
        imports, and function calls. Interface files (`.resi`) are flagged via
        ``File`` node ``extra["rescript_interface"]=True`` and skip call
        extraction since signatures have no call sites.
        """
        return _rescript_lang.parse(self, path, source)

    def _resolve_call_targets(
        self,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        file_path: str,
    ) -> list[EdgeInfo]:
        """Resolve bare call targets to qualified names using same-file definitions.

        After parsing, CALLS edges store bare function names (e.g. ``FirebaseAuth``)
        as targets. This method builds a symbol table from the parsed nodes and
        qualifies any bare target that matches a local definition, so that
        ``callers_of`` / ``callees_of`` queries produce correct results.

        External calls (names not defined in this file) remain bare.
        """
        # Build symbol table: bare_name -> qualified_name
        symbols: dict[str, str] = {}
        for node in nodes:
            if node.kind in ("Function", "Class", "Type", "Test"):
                bare = node.name
                qualified = self._qualify(bare, file_path, node.parent_name)
                if bare not in symbols:
                    symbols[bare] = qualified

        resolved: list[EdgeInfo] = []
        for edge in edges:
            if edge.kind in ("CALLS", "REFERENCES") and "::" not in edge.target:
                if edge.target in symbols:
                    edge = EdgeInfo(
                        kind=edge.kind,
                        source=edge.source,
                        target=symbols[edge.target],
                        file_path=edge.file_path,
                        line=edge.line,
                        extra=edge.extra,
                    )
            resolved.append(edge)
        return resolved

    _MAX_AST_DEPTH = 180  # Guard against pathologically nested source files
    _MAX_TEST_DESCRIPTION_LEN = 200  # Cap test description length in node names

    def _get_test_description(self, call_node, source: bytes) -> Optional[str]:
        """Extract the first string argument from a test runner call node."""
        for child in call_node.children:
            if child.type == "arguments":
                for arg in child.children:
                    if arg.type in ("string", "template_string"):
                        raw = arg.text.decode("utf-8", errors="replace")
                        stripped = raw.strip("'\"`")
                        normalized = re.sub(r"\s+", " ", stripped).strip()
                        if len(normalized) > self._MAX_TEST_DESCRIPTION_LEN:
                            normalized = normalized[: self._MAX_TEST_DESCRIPTION_LEN]
                        return normalized
        return None

    def _extract_from_tree(
        self,
        root,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str] = None,
        enclosing_func: Optional[str] = None,
        import_map: Optional[dict[str, str]] = None,
        defined_names: Optional[set[str]] = None,
        _depth: int = 0,
    ) -> None:
        """Recursively walk the AST and extract nodes/edges."""
        if _depth > self._MAX_AST_DEPTH:
            return
        class_types = set(_CLASS_TYPES.get(language, []))
        func_types = set(_FUNCTION_TYPES.get(language, []))
        import_types = set(_IMPORT_TYPES.get(language, []))
        call_types = set(_CALL_TYPES.get(language, []))

        for child in root.children:
            node_type = child.type

            # --- Language-specific constructs ---
            if handler := _SPECIAL_HANDLERS.get(language):
                if handler(
                    self,
                    child,
                    node_type,
                    source,
                    language,
                    file_path,
                    nodes,
                    edges,
                    enclosing_class,
                    enclosing_func,
                    import_map,
                    defined_names,
                    _depth,
                ):
                    continue

            # --- JS/TS variable-assigned functions (const foo = () => {}) ---
            if (
                language in ("javascript", "typescript", "tsx")
                and node_type in ("lexical_declaration", "variable_declaration")
                and self._extract_js_var_functions(
                    child,
                    source,
                    language,
                    file_path,
                    nodes,
                    edges,
                    enclosing_class,
                    enclosing_func,
                    import_map,
                    defined_names,
                    _depth,
                )
            ):
                continue

            # --- Classes ---
            if node_type in class_types and self._extract_classes(
                child,
                source,
                language,
                file_path,
                nodes,
                edges,
                enclosing_class,
                import_map,
                defined_names,
                _depth,
            ):
                continue

            # --- JS/TS class field arrow functions (handler = () => {}) ---
            if (
                language in ("javascript", "typescript", "tsx")
                and node_type == "public_field_definition"
                and self._extract_js_field_function(
                    child,
                    source,
                    language,
                    file_path,
                    nodes,
                    edges,
                    enclosing_class,
                    enclosing_func,
                    import_map,
                    defined_names,
                    _depth,
                )
            ):
                continue

            # --- Functions ---
            if node_type in func_types and self._extract_functions(
                child,
                source,
                language,
                file_path,
                nodes,
                edges,
                enclosing_class,
                enclosing_func,
                import_map,
                defined_names,
                _depth,
            ):
                continue

            # --- Imports ---
            if node_type in import_types:
                self._extract_imports(
                    child,
                    language,
                    source,
                    file_path,
                    edges,
                )
                # Ruby (and R) share "call" across import_types and call_types:
                # require() is a call node, but so are ordinary method calls.
                # For those languages, fall through to call/bridge handling so
                # non-require call nodes still get bridge detection. All other
                # languages use distinct node types for imports vs calls, so
                # the continue is always safe there.
                if node_type not in call_types:
                    continue

            # --- Calls ---
            if node_type in call_types:
                if self._extract_calls(
                    child,
                    source,
                    language,
                    file_path,
                    nodes,
                    edges,
                    enclosing_class,
                    enclosing_func,
                    import_map,
                    defined_names,
                    _depth,
                ):
                    continue

            # --- JSX component invocations ---
            if language in ("javascript", "typescript", "tsx") and node_type in (
                "jsx_opening_element",
                "jsx_self_closing_element",
            ):
                self._extract_jsx_component_call(
                    child,
                    language,
                    file_path,
                    edges,
                    enclosing_class,
                    enclosing_func,
                    import_map,
                    defined_names,
                )

            # --- Value references (function-as-value in maps, arrays, args) ---
            self._extract_value_references(
                child,
                node_type,
                source,
                language,
                file_path,
                edges,
                enclosing_class,
                enclosing_func,
                import_map,
                defined_names,
            )

            # Recurse for other node types
            self._extract_from_tree(
                child,
                source,
                language,
                file_path,
                nodes,
                edges,
                enclosing_class=enclosing_class,
                enclosing_func=enclosing_func,
                import_map=import_map,
                defined_names=defined_names,
                _depth=_depth + 1,
            )

    # Elixir helpers moved to languages/elixir.py; kept as shims for any
    # external call sites.
    def _elixir_call_identifier(self, node) -> Optional[str]:  # pragma: no cover
        from .languages import elixir as _elixir_lang

        return _elixir_lang._elixir_call_identifier(node)

    def _elixir_module_name(self, arguments) -> Optional[str]:  # pragma: no cover
        from .languages import elixir as _elixir_lang

        return _elixir_lang._elixir_module_name(arguments)

    def _elixir_function_name_and_params(
        self,
        arguments,
        source: bytes,
    ) -> tuple[Optional[str], Optional[str]]:  # pragma: no cover
        from .languages import elixir as _elixir_lang

        return _elixir_lang._elixir_function_name_and_params(arguments, source)

    def _extract_elixir_constructs(
        self,
        node,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
        _depth: int,
    ) -> bool:
        from .languages import elixir as _elixir_lang

        return _elixir_lang._extract_elixir_constructs(
            self,
            node,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class,
            enclosing_func,
            import_map,
            defined_names,
            _depth,
        )

    def _extract_bash_source_command(
        self,
        node,
        file_path: str,
        edges: list[EdgeInfo],
    ) -> bool:
        from .languages import bash as _bash_lang

        return _bash_lang._extract_bash_source_command(self, node, file_path, edges)

    def _extract_dart_calls_from_children(
        self,
        parent,
        source: bytes,
        file_path: str,
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
    ) -> None:
        from .languages import dart as _dart_lang

        _dart_lang._extract_dart_calls_from_children(
            self,
            parent,
            source,
            file_path,
            edges,
            enclosing_class,
            enclosing_func,
        )

    def _extract_r_constructs(
        self,
        child,
        node_type: str,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
    ) -> bool:
        from .languages import r as _r_lang

        return _r_lang._extract_r_constructs(
            self,
            child,
            node_type,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class,
            enclosing_func,
            import_map,
            defined_names,
        )

    # Julia helpers moved to languages/julia.py

    def _julia_short_func_name(self, call_expr) -> Optional[str]:
        from .languages import julia as _julia_lang

        return _julia_lang._julia_short_func_name(call_expr)

    def _julia_string_arg(self, call_expr) -> Optional[str]:
        from .languages import julia as _julia_lang

        return _julia_lang._julia_string_arg(call_expr)

    def _julia_call_first_identifier(self, call_expr) -> Optional[str]:
        from .languages import julia as _julia_lang

        return _julia_lang._julia_call_first_identifier(call_expr)

    def _julia_qualified_function_owner(self, node) -> Optional[str]:
        from .languages import julia as _julia_lang

        return _julia_lang._julia_qualified_function_owner(node)

    def _extract_julia_constructs(
        self,
        child,
        node_type: str,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
        _depth: int,
    ) -> bool:
        from .languages import julia as _julia_lang

        return _julia_lang._extract_julia_constructs(
            self,
            child,
            node_type,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class,
            enclosing_func,
            import_map,
            defined_names,
            _depth,
        )

    # ------------------------------------------------------------------
    # Lua-specific helpers (moved to languages/lua.py)
    # ------------------------------------------------------------------

    def _extract_lua_constructs(
        self,
        child,
        node_type: str,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
        _depth: int,
    ) -> bool:
        from .languages import lua as _lua_lang

        return _lua_lang._extract_lua_constructs(
            self,
            child,
            node_type,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class,
            enclosing_func,
            import_map,
            defined_names,
            _depth,
        )

    def _handle_lua_variable_declaration(self, *args, **kwargs) -> bool:
        from .languages import lua as _lua_lang

        return _lua_lang._handle_lua_variable_declaration(self, *args, **kwargs)

    def _handle_lua_table_function(self, *args, **kwargs) -> bool:
        from .languages import lua as _lua_lang

        return _lua_lang._handle_lua_table_function(self, *args, **kwargs)

    @staticmethod
    def _lua_get_require_target(call_node) -> Optional[str]:
        from .languages import lua as _lua_lang

        return _lua_lang._lua_get_require_target(call_node)

    # ------------------------------------------------------------------
    # JS/TS: variable-assigned functions  (const foo = () => {})
    # ------------------------------------------------------------------

    _JS_FUNC_VALUE_TYPES = frozenset(
        {"arrow_function", "function_expression", "function"},
    )

    def _extract_js_var_functions(
        self,
        child,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
        _depth: int,
    ) -> bool:
        """Handle JS/TS variable declarations that assign functions.

        Patterns handled:
          const foo = () => {}
          let bar = function() {}
          export const baz = (x: number): string => x.toString()

        Returns True if at least one function was extracted from the
        declaration, so the caller can skip generic recursion.
        """
        handled = False
        for declarator in child.children:
            if declarator.type != "variable_declarator":
                continue

            # Find identifier and function value
            var_name = None
            func_node = None
            for sub in declarator.children:
                if sub.type == "identifier" and var_name is None:
                    var_name = sub.text.decode("utf-8", errors="replace")
                elif sub.type in self._JS_FUNC_VALUE_TYPES:
                    func_node = sub

            if not var_name or not func_node:
                continue

            is_test = _is_test_function(var_name, file_path)
            kind = "Test" if is_test else "Function"
            qualified = self._qualify(var_name, file_path, enclosing_class)
            params = self._get_params(func_node, language, source)
            ret_type = self._get_return_type(func_node, language, source)

            nodes.append(
                NodeInfo(
                    kind=kind,
                    name=var_name,
                    file_path=file_path,
                    line_start=child.start_point[0] + 1,
                    line_end=child.end_point[0] + 1,
                    language=language,
                    parent_name=enclosing_class,
                    params=params,
                    return_type=ret_type,
                    is_test=is_test,
                )
            )
            container = (
                self._qualify(enclosing_class, file_path, None) if enclosing_class else file_path
            )
            edges.append(
                EdgeInfo(
                    kind="CONTAINS",
                    source=container,
                    target=qualified,
                    file_path=file_path,
                    line=child.start_point[0] + 1,
                )
            )

            # Recurse into the function body for calls
            self._extract_from_tree(
                func_node,
                source,
                language,
                file_path,
                nodes,
                edges,
                enclosing_class=enclosing_class,
                enclosing_func=var_name,
                import_map=import_map,
                defined_names=defined_names,
                _depth=_depth + 1,
            )
            handled = True

        if not handled:
            # Not a function assignment — let generic recursion handle it
            return False
        return True

    def _extract_js_field_function(
        self,
        child,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
        _depth: int,
    ) -> bool:
        """Handle class field arrow functions: handler = (e) => { ... }"""
        prop_name = None
        func_node = None
        for sub in child.children:
            if sub.type == "property_identifier" and prop_name is None:
                prop_name = sub.text.decode("utf-8", errors="replace")
            elif sub.type in self._JS_FUNC_VALUE_TYPES:
                func_node = sub

        if not prop_name or not func_node:
            return False

        is_test = _is_test_function(prop_name, file_path)
        kind = "Test" if is_test else "Function"
        qualified = self._qualify(prop_name, file_path, enclosing_class)
        params = self._get_params(func_node, language, source)

        nodes.append(
            NodeInfo(
                kind=kind,
                name=prop_name,
                file_path=file_path,
                line_start=child.start_point[0] + 1,
                line_end=child.end_point[0] + 1,
                language=language,
                parent_name=enclosing_class,
                params=params,
                is_test=is_test,
            )
        )
        container = (
            self._qualify(enclosing_class, file_path, None) if enclosing_class else file_path
        )
        edges.append(
            EdgeInfo(
                kind="CONTAINS",
                source=container,
                target=qualified,
                file_path=file_path,
                line=child.start_point[0] + 1,
            )
        )

        self._extract_from_tree(
            func_node,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class=enclosing_class,
            enclosing_func=prop_name,
            import_map=import_map,
            defined_names=defined_names,
            _depth=_depth + 1,
        )
        return True

    def _extract_classes(
        self,
        child,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
        _depth: int,
    ) -> bool:
        """Extract a class definition node and its inheritance edges.

        Returns True if the child was handled (class with a name found).
        """
        name = self._get_name(child, language, "class")
        if not name:
            return False

        extra: dict = {}
        _type_role, _is_abstract, _is_contract = _resolve_type_role(child, language)
        extra["type_role"] = _type_role
        if _is_abstract:
            extra["is_abstract"] = True
        if _is_contract:
            extra["is_contract"] = True

        # Swift: also preserve swift_kind for backward compatibility.
        if language == "swift":
            if child.type == "class_declaration":
                _swift_keywords = {"class", "struct", "enum", "actor", "extension"}
                for kw_child in child.children:
                    kw_text = kw_child.text.decode("utf-8", errors="replace")
                    if kw_text in _swift_keywords:
                        extra["swift_kind"] = kw_text
                        break
            elif child.type == "protocol_declaration":
                extra["swift_kind"] = "protocol"

        node = NodeInfo(
            kind="Class",
            name=name,
            file_path=file_path,
            line_start=child.start_point[0] + 1,
            line_end=child.end_point[0] + 1,
            language=language,
            parent_name=enclosing_class,
            extra=extra,
        )
        nodes.append(node)

        # CONTAINS edge
        edges.append(
            EdgeInfo(
                kind="CONTAINS",
                source=file_path,
                target=self._qualify(name, file_path, enclosing_class),
                file_path=file_path,
                line=child.start_point[0] + 1,
            )
        )

        # Inheritance / implementation edges
        bases = self._get_bases(child, language, source)
        for base_name, base_role in bases:
            edge_kind = "IMPLEMENTS" if base_role == "implements" else "INHERITS"
            edges.append(
                EdgeInfo(
                    kind=edge_kind,
                    source=self._qualify(
                        name,
                        file_path,
                        enclosing_class,
                    ),
                    target=base_name,
                    file_path=file_path,
                    line=child.start_point[0] + 1,
                    extra={"relationship_role": base_role, "syntax_source": child.type},
                )
            )

        # Recurse into class body
        self._extract_from_tree(
            child,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class=name,
            enclosing_func=None,
            import_map=import_map,
            defined_names=defined_names,
            _depth=_depth + 1,
        )
        return True

    def _extract_functions(
        self,
        child,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
        _depth: int,
    ) -> bool:
        """Extract a function/method definition node.

        Returns True if the child was handled (function with a name found).
        """
        name = self._get_name(child, language, "function")
        if not name:
            return False

        if language == "julia" and enclosing_func:
            enclosing_class = (
                f"{enclosing_class}.{enclosing_func}" if enclosing_class else enclosing_func
            )

        # Go methods: attach to their receiver type as the enclosing class,
        # so `func (s *T) Foo()` becomes a member of T rather than a
        # top-level function. See: #190
        if language == "go" and child.type == "method_declaration":
            receiver_type = self._get_go_receiver_type(child)
            if receiver_type:
                enclosing_class = receiver_type

        # Extract annotations/decorators for test detection
        decorators: tuple[str, ...] = ()
        deco_list: list[str] = []
        for sub in child.children:
            # Java/Kotlin/C#: annotations inside a modifiers child
            if sub.type == "modifiers":
                for mod in sub.children:
                    if mod.type in ("annotation", "marker_annotation"):
                        text = mod.text.decode("utf-8", errors="replace")
                        deco_list.append(text.lstrip("@").strip())
        # Python: check parent decorated_definition for decorator siblings
        if child.parent and child.parent.type == "decorated_definition":
            for sib in child.parent.children:
                if sib.type == "decorator":
                    text = sib.text.decode("utf-8", errors="replace")
                    deco_list.append(text.lstrip("@").strip())
        if deco_list:
            decorators = tuple(deco_list)

        is_test = _is_test_function(name, file_path, decorators)
        kind = "Test" if is_test else "Function"
        qualified = self._qualify(name, file_path, enclosing_class)
        params = self._get_params(child, language, source)
        ret_type = self._get_return_type(child, language, source)

        node = NodeInfo(
            kind=kind,
            name=name,
            file_path=file_path,
            line_start=child.start_point[0] + 1,
            line_end=child.end_point[0] + 1,
            language=language,
            parent_name=enclosing_class,
            params=params,
            return_type=ret_type,
            is_test=is_test,
        )
        nodes.append(node)

        # CONTAINS edge
        container = (
            self._qualify(enclosing_class, file_path, None) if enclosing_class else file_path
        )
        edges.append(
            EdgeInfo(
                kind="CONTAINS",
                source=container,
                target=qualified,
                file_path=file_path,
                line=child.start_point[0] + 1,
            )
        )

        if language == "julia":
            owner = self._julia_qualified_function_owner(child)
            if owner:
                edges.append(
                    EdgeInfo(
                        kind="REFERENCES",
                        source=qualified,
                        target=owner,
                        file_path=file_path,
                        line=child.start_point[0] + 1,
                    )
                )

        # Solidity: modifier invocations on functions -> CALLS edges
        if language == "solidity":
            for sub in child.children:
                if sub.type == "modifier_invocation":
                    for ident in sub.children:
                        if ident.type == "identifier":
                            edges.append(
                                EdgeInfo(
                                    kind="CALLS",
                                    source=qualified,
                                    target=ident.text.decode(
                                        "utf-8",
                                        errors="replace",
                                    ),
                                    file_path=file_path,
                                    line=sub.start_point[0] + 1,
                                )
                            )
                            break

        # Recurse to find calls inside the function
        self._extract_from_tree(
            child,
            source,
            language,
            file_path,
            nodes,
            edges,
            enclosing_class=enclosing_class,
            enclosing_func=name,
            import_map=import_map,
            defined_names=defined_names,
            _depth=_depth + 1,
        )
        return True

    def _extract_imports(
        self,
        child,
        language: str,
        source: bytes,
        file_path: str,
        edges: list[EdgeInfo],
    ) -> None:
        """Extract import edges from an import statement node."""
        imports = self._extract_import(child, language, source)
        for imp_target in imports:
            resolved = self._resolve_module_to_file(
                imp_target,
                file_path,
                language,
            )
            edges.append(
                EdgeInfo(
                    kind="IMPORTS_FROM",
                    source=file_path,
                    target=resolved if resolved else imp_target,
                    file_path=file_path,
                    line=child.start_point[0] + 1,
                )
            )

    def _extract_calls(
        self,
        child,
        source: bytes,
        language: str,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
        _depth: int,
    ) -> bool:
        """Extract call expressions, including test runner special cases.

        Returns True if the child was fully handled (test runner call that
        should skip default recursion). Returns False if the caller should
        continue to Solidity handling and default recursion.
        """
        call_name = self._get_call_name(child, language, source)

        # For member expressions like describe.only / it.skip / test.each,
        # resolve the base call name so those are treated as test runner
        # calls.
        effective_call_name = call_name
        if (
            call_name
            and language in ("javascript", "typescript", "tsx")
            and _is_test_file(file_path)
            and call_name not in _TEST_RUNNER_NAMES
        ):
            effective_call_name = self._get_base_call_name(child, source) or call_name

        # Special handling: test runner calls in test files -> Test nodes
        if (
            effective_call_name
            and language in ("javascript", "typescript", "tsx")
            and _is_test_file(file_path)
            and effective_call_name in _TEST_RUNNER_NAMES
        ):
            test_desc = self._get_test_description(child, source)
            line_no = child.start_point[0] + 1
            synthetic_base = (
                f"{effective_call_name}:{test_desc}" if test_desc else effective_call_name
            )
            synthetic_name = f"{synthetic_base}@L{line_no}"
            qualified = self._qualify(
                synthetic_name,
                file_path,
                enclosing_class,
            )

            nodes.append(
                NodeInfo(
                    kind="Test",
                    name=synthetic_name,
                    file_path=file_path,
                    line_start=child.start_point[0] + 1,
                    line_end=child.end_point[0] + 1,
                    language=language,
                    parent_name=enclosing_class,
                    is_test=True,
                )
            )

            # CONTAINS edge: parent -> this test
            container = (
                self._qualify(
                    enclosing_func,
                    file_path,
                    enclosing_class,
                )
                if enclosing_func
                else file_path
            )
            edges.append(
                EdgeInfo(
                    kind="CONTAINS",
                    source=container,
                    target=qualified,
                    file_path=file_path,
                    line=child.start_point[0] + 1,
                )
            )

            # Recurse into the call's children (the arrow function body)
            self._extract_from_tree(
                child,
                source,
                language,
                file_path,
                nodes,
                edges,
                enclosing_class=enclosing_class,
                enclosing_func=synthetic_name,
                import_map=import_map,
                defined_names=defined_names,
                _depth=_depth + 1,
            )
            return True

        if call_name:
            # Module-scope calls (no enclosing function) are attributed to
            # the File node. Matches the existing convention for CONTAINS
            # edges and _extract_value_references. Without this fallback,
            # any function called only from top-level script glue, CLI
            # entrypoints, or Jupyter/Databricks notebook cells is flagged
            # as dead by find_dead_code.
            caller = (
                self._qualify(enclosing_func, file_path, enclosing_class)
                if enclosing_func
                else file_path
            )
            target = self._resolve_call_target(
                call_name,
                file_path,
                language,
                import_map or {},
                defined_names or set(),
            )
            edges.append(
                EdgeInfo(
                    kind="CALLS",
                    source=caller,
                    target=target,
                    file_path=file_path,
                    line=child.start_point[0] + 1,
                )
            )

        # --- Cross-language bridge detection (any language with patterns) ---
        if language in _BRIDGE_PATTERNS and child.children:
            _bridge_caller = (
                self._qualify(enclosing_func, file_path, enclosing_class)
                if enclosing_func
                else file_path
            )
            edges.extend(
                self._detect_cross_language_bridge(child, language, file_path, _bridge_caller)
            )

        return False

    def _detect_cross_language_bridge(
        self,
        call_node,
        language: str,
        file_path: str,
        caller: str,
    ) -> list[EdgeInfo]:
        return bridges.detect_cross_language_bridge(call_node, language, file_path, caller)

    def _extract_jsx_component_call(
        self,
        child,
        language: str,
        file_path: str,
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
    ) -> None:
        """Emit a synthetic CALLS edge for JSX component usage.

        React-style component invocations use JSX rather than ``call_expression``.
        Treat uppercase component tags such as ``<MarkdownMsg />`` as call-like
        edges so caller/impact queries can cross the JSX boundary. Intrinsic DOM
        tags (``<div>``) are ignored.

        Module-scope JSX (e.g. a top-level ``<App />`` render call) attributes
        to the File node.
        """
        target = self._resolve_jsx_component_target(
            child,
            language,
            file_path,
            import_map or {},
            defined_names or set(),
        )
        if not target:
            return

        caller = (
            self._qualify(enclosing_func, file_path, enclosing_class)
            if enclosing_func
            else file_path
        )
        edges.append(
            EdgeInfo(
                kind="CALLS",
                source=caller,
                target=target,
                file_path=file_path,
                line=child.start_point[0] + 1,
            )
        )

    def _resolve_jsx_component_target(
        self,
        node,
        language: str,
        file_path: str,
        import_map: dict[str, str],
        defined_names: set[str],
    ) -> Optional[str]:
        """Resolve a JSX component element to a call target."""
        component_ref = self._get_jsx_component_reference(node)
        if component_ref is None:
            return None

        base_name, component_name = component_ref
        if base_name is None:
            return self._resolve_call_target(
                component_name,
                file_path,
                language,
                import_map,
                defined_names,
            )

        if base_name in import_map:
            resolved = self._resolve_imported_symbol(
                component_name,
                import_map[base_name],
                file_path,
                language,
            )
            if resolved:
                return resolved

        return component_name

    # ------------------------------------------------------------------
    # Value-reference extraction (function-as-value patterns)
    # ------------------------------------------------------------------

    # AST node types that represent object literal key-value pairs.
    _PAIR_TYPES = frozenset({"pair"})

    # AST node types for array/list containers.
    _ARRAY_TYPES = frozenset({"array", "list"})

    # Names that are almost certainly not function references (constants,
    # common primitives).  All-uppercase identifiers and very short names
    # are excluded by a length/casing heuristic in the method itself.
    _VALUE_REF_SKIP_NAMES = frozenset(
        {
            "true",
            "false",
            "null",
            "undefined",
            "None",
            "True",
            "False",
            "self",
            "this",
            "cls",
            "super",
        }
    )

    def _extract_value_references(
        self,
        child,
        node_type: str,
        source: bytes,
        language: str,
        file_path: str,
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
    ) -> None:
        """Emit ``REFERENCES`` edges for function-as-value patterns.

        Detects identifiers in value positions that likely refer to
        functions — object literal values, map property assignments,
        array elements, and callback arguments.  This reduces false
        positives in dead-code detection for dispatch-map patterns
        like ``Record<string, Handler>``.

        Only emits edges when the identifier matches a locally defined
        name or an imported symbol, avoiding noise from arbitrary
        variable references.
        """
        imap = import_map or {}
        dnames = defined_names or set()

        # Use enclosing function as source, or the file path for module-scope code.
        if enclosing_func:
            caller = self._qualify(enclosing_func, file_path, enclosing_class)
        else:
            caller = file_path

        # --- JS/TS/Python: object literal pair values  { key: fnRef } ---
        if node_type in self._PAIR_TYPES:
            self._ref_from_pair(child, source, language, file_path, caller, edges, imap, dnames)
            return

        # --- JS/TS: shorthand property identifiers  { fnRef } ---
        if node_type == "shorthand_property_identifier" and language in (
            "javascript",
            "typescript",
            "tsx",
        ):
            name = child.text.decode("utf-8", errors="replace")
            self._emit_reference_if_known(
                name,
                language,
                file_path,
                caller,
                edges,
                imap,
                dnames,
                line=child.start_point[0] + 1,
            )
            return

        # --- JS/TS/Python: assignment with member/subscript LHS ---
        if node_type in ("assignment_expression", "augmented_assignment", "assignment"):
            self._ref_from_assignment(
                child,
                source,
                language,
                file_path,
                caller,
                edges,
                imap,
                dnames,
            )
            return

        # --- JS/TS/Python: array / list elements ---
        if node_type in self._ARRAY_TYPES:
            self._ref_from_array(child, source, language, file_path, caller, edges, imap, dnames)
            return

        # --- Callback arguments (identifier args inside call_expression) ---
        if node_type == "arguments":
            self._ref_from_arguments(
                child,
                source,
                language,
                file_path,
                caller,
                edges,
                imap,
                dnames,
            )

    def _emit_reference_if_known(
        self,
        name: str,
        language: str,
        file_path: str,
        caller: str,
        edges: list[EdgeInfo],
        import_map: dict[str, str],
        defined_names: set[str],
        line: int = 0,
    ) -> None:
        """Emit a ``REFERENCES`` edge if *name* is a known function/import."""
        if not name or name in self._VALUE_REF_SKIP_NAMES:
            return
        # Skip all-uppercase names (likely constants) and single-char names.
        if name.isupper() or len(name) <= 1:
            return
        # Must be a known local definition or import to be worth tracking.
        if name not in defined_names and name not in import_map:
            return

        target = self._resolve_call_target(
            name,
            file_path,
            language,
            import_map,
            defined_names,
        )
        edges.append(
            EdgeInfo(
                kind="REFERENCES",
                source=caller,
                target=target,
                file_path=file_path,
                line=line,
            )
        )

    def _ref_from_pair(
        self,
        pair_node,
        source: bytes,
        language: str,
        file_path: str,
        caller: str,
        edges: list[EdgeInfo],
        import_map: dict[str, str],
        defined_names: set[str],
    ) -> None:
        """Extract a REFERENCES edge from an object/dict literal pair value."""
        # pair children: key, ":", value
        children = pair_node.children
        # Find the value — it's the last meaningful child.
        value_node = None
        for ch in reversed(children):
            if ch.type not in (":", ",", "comment"):
                value_node = ch
                break
        if value_node is None:
            return
        if value_node.type == "identifier":
            name = value_node.text.decode("utf-8", errors="replace")
            self._emit_reference_if_known(
                name,
                language,
                file_path,
                caller,
                edges,
                import_map,
                defined_names,
                line=value_node.start_point[0] + 1,
            )

    def _ref_from_assignment(
        self,
        assign_node,
        source: bytes,
        language: str,
        file_path: str,
        caller: str,
        edges: list[EdgeInfo],
        import_map: dict[str, str],
        defined_names: set[str],
    ) -> None:
        """Extract REFERENCES from ``obj.key = fnRef`` or ``obj['key'] = fnRef``."""
        children = assign_node.children
        if len(children) < 3:
            return
        lhs = children[0]
        # LHS must be a member_expression or subscript_expression (map assignment).
        if lhs.type not in (
            "member_expression",
            "subscript_expression",
            "attribute",
            "subscript",
        ):
            return
        # RHS is the last non-punctuation child.
        rhs = None
        for ch in reversed(children):
            if ch.type not in ("=", ":", ",", "comment", "type_annotation"):
                rhs = ch
                break
        if rhs is None or rhs.type != "identifier":
            return
        name = rhs.text.decode("utf-8", errors="replace")
        self._emit_reference_if_known(
            name,
            language,
            file_path,
            caller,
            edges,
            import_map,
            defined_names,
            line=rhs.start_point[0] + 1,
        )

    def _ref_from_array(
        self,
        array_node,
        source: bytes,
        language: str,
        file_path: str,
        caller: str,
        edges: list[EdgeInfo],
        import_map: dict[str, str],
        defined_names: set[str],
    ) -> None:
        """Extract REFERENCES from array/list elements that are identifiers."""
        for ch in array_node.children:
            if ch.type == "identifier":
                name = ch.text.decode("utf-8", errors="replace")
                self._emit_reference_if_known(
                    name,
                    language,
                    file_path,
                    caller,
                    edges,
                    import_map,
                    defined_names,
                    line=ch.start_point[0] + 1,
                )

    def _ref_from_arguments(
        self,
        args_node,
        source: bytes,
        language: str,
        file_path: str,
        caller: str,
        edges: list[EdgeInfo],
        import_map: dict[str, str],
        defined_names: set[str],
    ) -> None:
        """Extract REFERENCES from identifier arguments (callbacks)."""
        for ch in args_node.children:
            if ch.type == "identifier":
                name = ch.text.decode("utf-8", errors="replace")
                self._emit_reference_if_known(
                    name,
                    language,
                    file_path,
                    caller,
                    edges,
                    import_map,
                    defined_names,
                    line=ch.start_point[0] + 1,
                )

    def _extract_solidity_constructs(
        self,
        child,
        node_type: str,
        source: bytes,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
    ) -> bool:
        from .languages import solidity as _solidity_lang

        return _solidity_lang._extract_solidity_constructs(
            self,
            child,
            node_type,
            source,
            file_path,
            nodes,
            edges,
            enclosing_class,
            enclosing_func,
        )

    def _strip_tf_string(self, value: str) -> str:
        from .languages import terraform as _terraform_lang

        return _terraform_lang._strip_tf_string(value)

    def _terraform_field_text(self, node, field_name: str) -> Optional[str]:
        from .languages import terraform as _terraform_lang

        return _terraform_lang._terraform_field_text(node, field_name)

    def _terraform_field_node(self, node, field_name: str):
        from .languages import terraform as _terraform_lang

        return _terraform_lang._terraform_field_node(node, field_name)

    def _terraform_collect_references(
        self, text: str, caller: str, file_path: str, line: int, edges: list[EdgeInfo]
    ) -> None:
        from .languages import terraform as _terraform_lang

        _terraform_lang._terraform_collect_references(text, caller, file_path, line, edges)

    def _terraform_collect_calls(
        self, text: str, caller: str, file_path: str, line: int, edges: list[EdgeInfo]
    ) -> None:
        from .languages import terraform as _terraform_lang

        _terraform_lang._terraform_collect_calls(text, caller, file_path, line, edges)

    def _terraform_scan_body(
        self, body_text: str, caller: str, file_path: str, line: int, edges: list[EdgeInfo]
    ) -> None:
        from .languages import terraform as _terraform_lang

        _terraform_lang._terraform_scan_body(body_text, caller, file_path, line, edges)

    def _get_terraform_defined_name(self, node) -> Optional[str]:
        from .languages import terraform as _terraform_lang

        return _terraform_lang._get_terraform_defined_name(node)

    def _extract_terraform_constructs(
        self,
        child,
        node_type: str,
        source: bytes,
        file_path: str,
        nodes: list[NodeInfo],
        edges: list[EdgeInfo],
        enclosing_class: Optional[str],
        enclosing_func: Optional[str],
        import_map: Optional[dict[str, str]],
        defined_names: Optional[set[str]],
        _depth: int,
    ) -> bool:
        from .languages import terraform as _terraform_lang

        return _terraform_lang._extract_terraform_constructs(
            self,
            child,
            node_type,
            source,
            file_path,
            nodes,
            edges,
            enclosing_class,
            enclosing_func,
            import_map,
            defined_names,
            _depth,
        )

    def _collect_file_scope(
        self,
        root,
        language: str,
        source: bytes,
    ) -> tuple[dict[str, str], set[str]]:
        """Pre-scan top-level AST to collect import mappings and defined names.

        Returns:
            (import_map, defined_names) where import_map maps imported names
            to their source module/path, and defined_names is the set of
            function/class names defined at file scope.
        """
        import_map: dict[str, str] = {}
        defined_names: set[str] = set()

        class_types = set(_CLASS_TYPES.get(language, []))
        func_types = set(_FUNCTION_TYPES.get(language, []))
        import_types = set(_IMPORT_TYPES.get(language, []))

        # Node types that wrap a class/function with decorators/annotations
        decorator_wrappers = {"decorated_definition", "decorator"}

        for child in root.children:
            node_type = child.type

            # Unwrap decorator wrappers to reach the inner definition
            target = child
            if node_type in decorator_wrappers:
                for inner in child.children:
                    if inner.type in func_types or inner.type in class_types:
                        target = inner
                        break
            elif (
                language in ("javascript", "typescript", "tsx") and node_type == "export_statement"
            ):
                for inner in child.children:
                    if inner.type in func_types or inner.type in class_types:
                        target = inner
                        break

            target_type = target.type

            # R: function names live on the left side of binary_operator
            if language == "r" and target_type == "binary_operator":
                r_children = target.children
                if (
                    len(r_children) >= 3
                    and r_children[0].type == "identifier"
                    and r_children[2].type == "function_definition"
                ):
                    name = r_children[0].text.decode("utf-8", errors="replace")
                    defined_names.add(name)
                    continue

            if language == "julia" and target_type == "assignment":
                lhs = target.children[0] if target.children else None
                if lhs is not None and lhs.type == "typed_expression":
                    for sub in lhs.children:
                        if sub.type == "call_expression":
                            lhs = sub
                            break
                if lhs is not None and lhs.type == "call_expression":
                    name = self._julia_short_func_name(lhs)
                    if name:
                        defined_names.add(name)
                        continue

            if language == "terraform":
                name = self._get_terraform_defined_name(target)
                if name:
                    defined_names.add(name)
                    continue

            # Collect defined function/class names
            if target_type in func_types or target_type in class_types:
                name = self._get_name(
                    target, language, "class" if target_type in class_types else "function"
                )
                if name:
                    defined_names.add(name)
                    continue

            if language in ("javascript", "typescript", "tsx") and node_type == "export_statement":
                self._collect_js_exported_local_names(child, defined_names)

            # Collect import mappings: imported_name → module_path
            if node_type in import_types:
                self._collect_import_names(child, language, source, import_map)

        return import_map, defined_names

    def _collect_js_exported_local_names(
        self,
        node,
        defined_names: set[str],
    ) -> None:
        """Collect locally exported JS/TS names from export statements."""
        for child in node.children:
            if child.type in ("lexical_declaration", "variable_declaration"):
                for sub in child.children:
                    if sub.type == "variable_declarator":
                        for part in sub.children:
                            if part.type == "identifier":
                                defined_names.add(
                                    part.text.decode("utf-8", errors="replace"),
                                )
                                break

    def _collect_import_names(
        self,
        node,
        language: str,
        source: bytes,
        import_map: dict[str, str],
    ) -> None:
        """Extract imported names and their source modules into import_map."""
        if language == "python":
            if node.type == "import_from_statement":
                # from X.Y import A, B → {A: X.Y, B: X.Y}
                module = None
                seen_import_keyword = False
                for child in node.children:
                    if child.type == "dotted_name" and not seen_import_keyword:
                        module = child.text.decode("utf-8", errors="replace")
                    elif child.type == "import":
                        seen_import_keyword = True
                    elif seen_import_keyword and module:
                        if child.type in ("identifier", "dotted_name"):
                            name = child.text.decode("utf-8", errors="replace")
                            import_map[name] = module
                        elif child.type == "aliased_import":
                            # from X import A as B → {B: X}
                            names = [
                                sub.text.decode("utf-8", errors="replace")
                                for sub in child.children
                                if sub.type in ("identifier", "dotted_name")
                            ]
                            # Last name is the alias (local name)
                            if names:
                                import_map[names[-1]] = module

        elif language in ("javascript", "typescript", "tsx"):
            # import { A, B } from './path' → {A: ./path, B: ./path}
            module = None
            for child in node.children:
                if child.type == "string":
                    module = child.text.decode("utf-8", errors="replace").strip("'\"")
            if module:
                for child in node.children:
                    if child.type == "import_clause":
                        self._collect_js_import_names(child, module, import_map)
        elif language == "julia":
            for child in node.children:
                if child.type == "identifier":
                    module = child.text.decode("utf-8", errors="replace")
                    import_map[module] = module
                elif child.type == "selected_import":
                    module = None
                    seen_colon = False
                    for sub in child.children:
                        if sub.type == ":":
                            seen_colon = True
                            continue
                        if not seen_colon:
                            if sub.type == "identifier":
                                module = sub.text.decode("utf-8", errors="replace")
                        elif sub.type == "identifier" and module:
                            imported = sub.text.decode("utf-8", errors="replace")
                            import_map[imported] = module

    def _collect_js_import_names(
        self,
        clause_node,
        module: str,
        import_map: dict[str, str],
    ) -> None:
        """Walk JS/TS import_clause to extract named and default imports."""
        for child in clause_node.children:
            if child.type == "identifier":
                # Default import
                import_map[child.text.decode("utf-8", errors="replace")] = module
            elif child.type == "namespace_import":
                for sub in child.children:
                    if sub.type == "identifier":
                        import_map[sub.text.decode("utf-8", errors="replace")] = module
                        break
            elif child.type == "named_imports":
                for spec in child.children:
                    if spec.type == "import_specifier":
                        # Could be: name or name as alias
                        names = [
                            s.text.decode("utf-8", errors="replace")
                            for s in spec.children
                            if s.type in ("identifier", "property_identifier")
                        ]
                        # Last identifier is the local name
                        if names:
                            import_map[names[-1]] = module

    def _resolve_module_to_file(
        self,
        module: str,
        file_path: str,
        language: str,
    ) -> Optional[str]:
        """Resolve a module/import path to an absolute file path.

        Uses self._module_file_cache to avoid repeated filesystem lookups.
        """
        caller_dir = str(Path(file_path).parent)
        cache_key = f"{language}:{caller_dir}:{module}"
        if cache_key in self._module_file_cache:
            return self._module_file_cache[cache_key]

        resolved = self._do_resolve_module(module, file_path, language)
        if len(self._module_file_cache) >= self._MODULE_CACHE_MAX:
            self._module_file_cache.clear()
        self._module_file_cache[cache_key] = resolved
        return resolved

    def _do_resolve_module(
        self,
        module: str,
        file_path: str,
        language: str,
    ) -> Optional[str]:
        """Language-aware module-to-file resolution."""
        caller_dir = Path(file_path).parent

        if language == "bash":
            # ``source ./lib.sh`` or ``source lib.sh`` — resolve relative
            # to the caller's directory. See: #197
            try:
                target = (caller_dir / module).resolve()
                if target.is_file():
                    return str(target)
            except (OSError, ValueError):
                pass
            return None

        if language == "python":
            if module.startswith("."):
                # Relative import: "." = same package, ".." = parent package, etc.
                leading_dots = len(module) - len(module.lstrip("."))
                remainder = module[leading_dots:]  # "" or "tools" or "tools.build"
                base = caller_dir
                for _ in range(leading_dots - 1):
                    base = base.parent
                if remainder:
                    rel = remainder.replace(".", "/")
                    candidates = [base / f"{rel}.py", base / rel / "__init__.py"]
                else:
                    candidates = [base / "__init__.py"]
                for c in candidates:
                    if c.is_file():
                        return str(c.resolve())
                return None

            rel_path = module.replace(".", "/")
            candidates = [rel_path + ".py", rel_path + "/__init__.py"]
            # Walk up from caller's directory to find the module file
            current = caller_dir
            while True:
                for candidate in candidates:
                    target = current / candidate
                    if target.is_file():
                        return str(target.resolve())
                if current == current.parent:
                    break
                current = current.parent

        elif language in ("javascript", "typescript", "tsx", "vue"):
            if module.startswith("."):
                # Relative import — resolve from caller's directory
                base = caller_dir / module
                extensions = [".ts", ".tsx", ".js", ".jsx", ".vue"]
                # Try exact path first (might already have extension)
                if base.is_file():
                    return str(base.resolve())
                # Try with extensions
                for ext in extensions:
                    target = base.with_suffix(ext)
                    if target.is_file():
                        return str(target.resolve())
                # Try index file in directory
                if base.is_dir():
                    for ext in extensions:
                        target = base / f"index{ext}"
                        if target.is_file():
                            return str(target.resolve())
            else:
                # Non-relative import — try tsconfig path alias resolution
                resolved = self._tsconfig_resolver.resolve_alias(module, file_path)
                if resolved:
                    return resolved

        elif language == "dart":
            if module.startswith("."):
                # Dart relative imports include the .dart extension
                base = caller_dir / module
                if base.is_file():
                    return str(base.resolve())
                # Fallback: try appending .dart
                target = base.with_suffix(".dart")
                if target.is_file():
                    return str(target.resolve())
            elif module.startswith("package:"):
                # ``package:<name>/<sub_path>`` — resolve to the current repo's
                # ``lib/<sub_path>`` iff a ``pubspec.yaml`` declaring that
                # package name is found in an ancestor directory. See: #87
                try:
                    uri_body = module[len("package:") :]
                    pkg_name, _, sub_path = uri_body.partition("/")
                    if not sub_path:
                        return None
                    pubspec_root = self._find_dart_pubspec_root(caller_dir, pkg_name)
                    if pubspec_root is not None:
                        target = pubspec_root / "lib" / sub_path
                        if target.is_file():
                            return str(target.resolve())
                except (OSError, ValueError):
                    return None
            # ``dart:core`` / ``dart:async`` etc. are SDK libraries we do
            # not track; fall through to return None.

        elif language == "java":
            # ``import com.example.pkg.ClassName;`` — convert dot-notation
            # to a relative path and walk up from the caller's directory to
            # find the source root.  Wildcards (``import pkg.*``) and static
            # member imports (``import static pkg.Class.member``) that don't
            # resolve as-is are retried after dropping the last segment
            # (the member name).
            if module.endswith(".*"):
                return None  # wildcard import — can't resolve to one file
            rel_path = module.replace(".", "/") + ".java"
            current = caller_dir
            while True:
                target = current / rel_path
                if target.is_file():
                    return str(target.resolve())
                if current == current.parent:
                    break
                current = current.parent
            # Static import: ``pkg.Class.member`` — strip member, try again
            dot = module.rfind(".")
            if dot > 0:
                class_module = module[:dot]
                rel_path2 = class_module.replace(".", "/") + ".java"
                current = caller_dir
                while True:
                    target = current / rel_path2
                    if target.is_file():
                        return str(target.resolve())
                    if current == current.parent:
                        break
                    current = current.parent

        return None

    def _find_dart_pubspec_root(
        self,
        start: Path,
        pkg_name: str,
    ) -> Optional[Path]:
        return _dart_lang.find_dart_pubspec_root(self, start, pkg_name)

    def _resolve_call_target(
        self,
        call_name: str,
        file_path: str,
        language: str,
        import_map: dict[str, str],
        defined_names: set[str],
    ) -> str:
        """Resolve a bare call name to a qualified target, with fallback."""
        if call_name in defined_names:
            return self._qualify(call_name, file_path, None)
        if call_name in import_map:
            resolved = self._resolve_imported_symbol(
                call_name,
                import_map[call_name],
                file_path,
                language,
            )
            if resolved:
                return resolved
        return call_name

    def _resolve_imported_symbol(
        self,
        symbol_name: str,
        module: str,
        file_path: str,
        language: str,
    ) -> Optional[str]:
        """Resolve an imported symbol to its defining qualified name when possible."""
        resolved = self._resolve_module_to_file(module, file_path, language)
        if not resolved:
            return None

        export_target = self._resolve_exported_symbol(resolved, symbol_name)
        if export_target:
            return export_target
        return self._qualify(symbol_name, resolved, None)

    def _resolve_exported_symbol(
        self,
        module_file: str,
        symbol_name: str,
        seen: Optional[set[tuple[str, str]]] = None,
    ) -> Optional[str]:
        """Resolve a JS/TS symbol through common re-export/barrel patterns."""
        cache_key = f"{module_file}::{symbol_name}"
        if cache_key in self._export_symbol_cache:
            return self._export_symbol_cache[cache_key]

        key = (module_file, symbol_name)
        if seen is None:
            seen = set()
        if key in seen:
            return None
        seen.add(key)

        path = Path(module_file)
        language = self.detect_language(path)
        if language not in ("javascript", "typescript", "tsx", "vue"):
            return None

        try:
            source = path.read_bytes()
        except (OSError, PermissionError):
            return None

        parser = self._get_parser(language)
        if not parser:
            return None

        tree = parser.parse(source)

        # Direct local definition/export in the module file.
        import_map, defined_names = self._collect_file_scope(
            tree.root_node,
            language,
            source,
        )
        if symbol_name in defined_names:
            result = self._qualify(symbol_name, module_file, None)
            self._export_symbol_cache[cache_key] = result
            return result

        for child in tree.root_node.children:
            if child.type != "export_statement":
                continue

            export_clause = None
            target_module = None
            has_star_export = False

            for sub in child.children:
                if sub.type == "export_clause":
                    export_clause = sub
                elif sub.type == "string":
                    target_module = sub.text.decode("utf-8", errors="replace").strip("'\"")
                elif sub.type == "*":
                    has_star_export = True

            # Re-exported names: export { Foo as Bar } from './x'
            if export_clause is not None:
                for spec in export_clause.children:
                    if spec.type != "export_specifier":
                        continue
                    names = [
                        part.text.decode("utf-8", errors="replace")
                        for part in spec.children
                        if part.type in ("identifier", "property_identifier")
                    ]
                    if not names:
                        continue
                    exported_name = names[-1]
                    original_name = names[0]
                    if exported_name != symbol_name:
                        continue
                    if target_module:
                        resolved_module = self._resolve_module_to_file(
                            target_module,
                            module_file,
                            language,
                        )
                        if resolved_module:
                            result = self._resolve_exported_symbol(
                                resolved_module,
                                original_name,
                                seen,
                            ) or self._qualify(original_name, resolved_module, None)
                            self._export_symbol_cache[cache_key] = result
                            return result
                    result = self._qualify(original_name, module_file, None)
                    self._export_symbol_cache[cache_key] = result
                    return result

            # Star re-export: export * from './x'
            if has_star_export and target_module:
                resolved_module = self._resolve_module_to_file(
                    target_module,
                    module_file,
                    language,
                )
                if resolved_module:
                    result = self._resolve_exported_symbol(
                        resolved_module,
                        symbol_name,
                        seen,
                    )
                    if result:
                        self._export_symbol_cache[cache_key] = result
                        return result

        self._export_symbol_cache[cache_key] = None
        return None

    def _qualify(self, name: str, file_path: str, enclosing_class: Optional[str]) -> str:
        """Create a qualified name: file_path::ClassName.name or file_path::name."""
        if enclosing_class:
            return f"{file_path}::{enclosing_class}.{name}"
        return f"{file_path}::{name}"

    def _get_name(self, node, language: str, kind: str) -> Optional[str]:
        """Extract the name from a class/function definition node."""
        # Dart: function_signature has a return-type node before the identifier;
        # search only for 'identifier' to avoid returning the return type name.
        if language == "dart" and node.type == "function_signature":
            for child in node.children:
                if child.type == "identifier":
                    return child.text.decode("utf-8", errors="replace")
            return None
        # Solidity: constructor and receive/fallback have no identifier child
        if language == "solidity":
            if node.type == "constructor_definition":
                return "constructor"
            if node.type == "fallback_receive_definition":
                for child in node.children:
                    if child.type in ("receive", "fallback"):
                        return child.text.decode("utf-8", errors="replace")
        # Lua/Luau: function_declaration names may be dot_index_expression or
        # method_index_expression (e.g. function Animal.new() / Animal:speak()).
        # Return only the method name; the table name is used as parent_name
        # in _extract_lua_constructs.
        if language in ("lua", "luau") and node.type == "function_declaration":
            for child in node.children:
                if child.type in ("dot_index_expression", "method_index_expression"):
                    # Last identifier child is the method name
                    for sub in reversed(child.children):
                        if sub.type == "identifier":
                            return sub.text.decode("utf-8", errors="replace")
                    return None
        # Perl: bareword for subroutine names, package for package names
        if language == "perl":
            for child in node.children:
                if child.type == "bareword":
                    return child.text.decode("utf-8", errors="replace")
                if child.type == "package" and child.text != b"package":
                    return child.text.decode("utf-8", errors="replace")
        # For C/C++/Objective-C: function names are inside
        # function_declarator / pointer_declarator. Check these first to
        # avoid matching the return type_identifier as the function name.
        if language in ("c", "cpp", "objc") and kind == "function":
            for child in node.children:
                if child.type in ("function_declarator", "pointer_declarator"):
                    result = self._get_name(child, language, kind)
                    if result:
                        return result

        # Objective-C method_definition: the method name is the first
        # ``identifier`` child (first part of the selector). Multi-part
        # selectors like ``- (void)add:(int)a to:(int)b`` keep ``add`` as
        # the canonical method name; later parts are keyword arguments.
        if language == "objc" and node.type == "method_definition":
            for child in node.children:
                if child.type == "identifier":
                    return child.text.decode("utf-8", errors="replace")

        # Bash function_definition: ``foo() { ... }`` — tree-sitter-bash
        # stores the function name as a ``word`` child, which the generic
        # loop below doesn't recognize.
        if language == "bash" and node.type == "function_definition":
            for child in node.children:
                if child.type == "word":
                    return child.text.decode("utf-8", errors="replace")
        # Go methods: tree-sitter-go uses field_identifier for the name
        # (e.g. func (s *T) MethodName(...) { }). Must run before the generic
        # loop, which would match the result type's type_identifier (e.g. int64).
        if language == "go" and node.type == "method_declaration":
            for child in node.children:
                if child.type == "field_identifier":
                    return child.text.decode("utf-8", errors="replace")
        # Java methods: tree-sitter-java puts type_identifier or generic_type
        # (return type) before identifier (method name).  Must run before
        # the generic loop, which would match the return type's
        # type_identifier (e.g. "String", "ConfigBean").
        # Constructors are fine — they have no return type node.
        # Kotlin is unaffected: its syntax places the name before the type.
        if language == "java" and node.type == "method_declaration":
            for child in node.children:
                if child.type == "identifier":
                    return child.text.decode("utf-8", errors="replace")
        # Swift extensions: name is inside user_type > type_identifier
        # (e.g. `extension MyClass: Protocol { ... }`)
        if language == "swift" and node.type == "class_declaration":
            for child in node.children:
                if child.type == "user_type":
                    for sub in child.children:
                        if sub.type == "type_identifier":
                            return sub.text.decode("utf-8", errors="replace")
        if language == "julia":
            if node.type in ("function_definition", "macro_definition"):
                for child in node.children:
                    if child.type == "signature":
                        call = child
                        for sub in call.children:
                            if sub.type == "where_expression":
                                call = sub
                                break
                        for sub in call.children:
                            if sub.type == "typed_expression":
                                call = sub
                                break
                        queue = [call]
                        while queue:
                            current = queue.pop(0)
                            if current.type == "call_expression":
                                for target in current.children:
                                    if target.type == "identifier":
                                        return target.text.decode("utf-8", errors="replace")
                                    if target.type == "field_expression":
                                        for ident in reversed(target.children):
                                            if ident.type == "identifier":
                                                return ident.text.decode(
                                                    "utf-8",
                                                    errors="replace",
                                                )
                                    if target.type == "parametrized_type_expression":
                                        for p in target.children:
                                            if p.type == "identifier":
                                                return p.text.decode(
                                                    "utf-8",
                                                    errors="replace",
                                                )
                                return None
                            queue.extend(list(current.children))
                return None
            if node.type in ("struct_definition", "abstract_definition"):
                for child in node.children:
                    if child.type == "type_head":
                        for sub in child.children:
                            if sub.type == "identifier":
                                return sub.text.decode("utf-8", errors="replace")
                        for sub in child.children:
                            if sub.type == "binary_expression":
                                for ident in sub.children:
                                    if ident.type == "identifier":
                                        return ident.text.decode("utf-8", errors="replace")
                                    if ident.type == "parametrized_type_expression":
                                        for p in ident.children:
                                            if p.type == "identifier":
                                                return p.text.decode(
                                                    "utf-8",
                                                    errors="replace",
                                                )
                                        return None
                                return None
                        for sub in child.children:
                            if sub.type == "parametrized_type_expression":
                                for p in sub.children:
                                    if p.type == "identifier":
                                        return p.text.decode("utf-8", errors="replace")
                                return None
                return None
        # Most languages use a 'name' child
        for child in node.children:
            if child.type in (
                "identifier",
                "name",
                "type_identifier",
                "property_identifier",
                "simple_identifier",
                "constant",
            ):
                return child.text.decode("utf-8", errors="replace")
        # For Go type declarations, look for type_spec
        if language == "go" and node.type == "type_declaration":
            for child in node.children:
                if child.type == "type_spec":
                    return self._get_name(child, language, kind)
        return None

    def _get_go_receiver_type(self, node) -> Optional[str]:
        """Extract the receiver type from a Go method_declaration.

        For ``func (s *T) Foo() {...}`` returns ``"T"``. For ``func (T) Foo()``
        also returns ``"T"``. Returns None if no receiver is present.

        The receiver is always the first ``parameter_list`` child of a
        Go ``method_declaration`` and contains a single ``parameter_declaration``
        whose type is either a ``type_identifier`` or a ``pointer_type``
        wrapping one. See: #190
        """
        for child in node.children:
            if child.type != "parameter_list":
                continue
            for param in child.children:
                if param.type != "parameter_declaration":
                    continue
                for sub in param.children:
                    if sub.type == "type_identifier":
                        return sub.text.decode("utf-8", errors="replace")
                    if sub.type == "pointer_type":
                        for ptr_child in sub.children:
                            if ptr_child.type == "type_identifier":
                                return ptr_child.text.decode("utf-8", errors="replace")
            # First parameter_list is always the receiver; stop searching.
            return None
        return None

    def _get_params(self, node, language: str, source: bytes) -> Optional[str]:
        """Extract parameter list as a string."""
        for child in node.children:
            param_types = (
                "parameters",
                "formal_parameters",
                "parameter_list",
                "formal_parameter_list",
            )
            if child.type in param_types:
                return child.text.decode("utf-8", errors="replace")
        # Solidity: parameters are direct children between ( and )
        if language == "solidity":
            params = [
                c.text.decode("utf-8", errors="replace")
                for c in node.children
                if c.type == "parameter"
            ]
            if params:
                return f"({', '.join(params)})"
        return None

    def _get_return_type(self, node, language: str, source: bytes) -> Optional[str]:
        """Extract return type annotation if present."""
        for child in node.children:
            if child.type in ("type", "return_type", "type_annotation", "return_type_definition"):
                return child.text.decode("utf-8", errors="replace")
        # Python: look for -> annotation
        if language == "python":
            for i, child in enumerate(node.children):
                if child.type == "->" and i + 1 < len(node.children):
                    return node.children[i + 1].text.decode("utf-8", errors="replace")
        return None

    def _get_bases(self, node, language: str, source: bytes) -> list[tuple[str, str]]:
        """Extract base classes and interfaces with relationship roles.

        Returns list of (name, role) tuples where role is "extends" or
        "implements". Languages that cannot distinguish the two use "extends".
        """
        bases: list[tuple[str, str]] = []
        if language == "python":
            for child in node.children:
                if child.type == "argument_list":
                    for arg in child.children:
                        if arg.type in ("identifier", "attribute"):
                            bases.append((arg.text.decode("utf-8", errors="replace"), "extends"))
        elif language == "java":
            # Java: superclass -> extends, super_interfaces -> implements.
            for child in node.children:
                if child.type == "superclass":
                    for sub in child.children:
                        if sub.type in ("type_identifier", "generic_type"):
                            bases.append((sub.text.decode("utf-8", errors="replace"), "extends"))
                elif child.type == "super_interfaces":
                    for sub in child.children:
                        if sub.type == "type_list":
                            for ident in sub.children:
                                if ident.type in ("type_identifier", "generic_type"):
                                    bases.append(
                                        (
                                            ident.text.decode("utf-8", errors="replace"),
                                            "implements",
                                        )
                                    )
        elif language in ("csharp", "kotlin"):
            # C#/Kotlin: map known "implements" node types; fall back to extends.
            _implements_types = frozenset(
                {
                    "super_interfaces",
                    "implements_type",
                    "delegation_specifier",
                }
            )
            for child in node.children:
                if child.type in (
                    "superclass",
                    "super_interfaces",
                    "extends_type",
                    "implements_type",
                    "type_identifier",
                    "supertype",
                    "delegation_specifier",
                ):
                    role = "implements" if child.type in _implements_types else "extends"
                    text = child.text.decode("utf-8", errors="replace")
                    bases.append((text, role))
        elif language == "scala":
            # Scala: first type in extends_clause is the superclass; remaining
            # entries (with-clause types) are treated as implements.
            for child in node.children:
                if child.type == "extends_clause":
                    first = True
                    for sub in child.children:
                        if sub.type == "type_identifier":
                            role = "extends" if first else "implements"
                            bases.append((sub.text.decode("utf-8", errors="replace"), role))
                            first = False
                        elif sub.type == "generic_type":
                            for ident in sub.children:
                                if ident.type == "type_identifier":
                                    role = "extends" if first else "implements"
                                    bases.append(
                                        (ident.text.decode("utf-8", errors="replace"), role)
                                    )
                                    first = False
                                    break
        elif language == "cpp":
            # C++: no language-level extends/implements distinction.
            for child in node.children:
                if child.type == "base_class_clause":
                    for sub in child.children:
                        if sub.type == "type_identifier":
                            bases.append((sub.text.decode("utf-8", errors="replace"), "extends"))
        elif language in ("typescript", "javascript", "tsx"):
            # TS/JS: extends_clause -> extends, implements_clause -> implements.
            # Both can appear as direct children or inside a class_heritage node.
            def _collect_ts_heritage(parent):
                for child in parent.children:
                    if child.type in ("extends_clause", "implements_clause"):
                        role = "implements" if child.type == "implements_clause" else "extends"
                        for sub in child.children:
                            if sub.type in ("identifier", "type_identifier", "nested_identifier"):
                                bases.append((sub.text.decode("utf-8", errors="replace"), role))
                    elif child.type == "class_heritage":
                        _collect_ts_heritage(child)

            _collect_ts_heritage(node)
        elif language == "solidity":
            # Solidity: contract Foo is Bar, Baz { ... }
            for child in node.children:
                if child.type == "inheritance_specifier":
                    for sub in child.children:
                        if sub.type == "user_defined_type":
                            for ident in sub.children:
                                if ident.type == "identifier":
                                    bases.append(
                                        (
                                            ident.text.decode("utf-8", errors="replace"),
                                            "extends",
                                        )
                                    )
        elif language == "go":
            # Embedded structs / interface composition — no extends/implements split.
            for child in node.children:
                if child.type == "type_spec":
                    for sub in child.children:
                        if sub.type in ("struct_type", "interface_type"):
                            for field_node in sub.children:
                                if field_node.type == "field_declaration_list":
                                    for f in field_node.children:
                                        if f.type == "type_identifier":
                                            bases.append(
                                                (
                                                    f.text.decode("utf-8", errors="replace"),
                                                    "extends",
                                                )
                                            )
        elif language == "dart":
            # Dart: superclass/mixins -> extends, interfaces -> implements.
            for child in node.children:
                if child.type == "superclass":
                    for sub in child.children:
                        if sub.type == "type_identifier":
                            bases.append((sub.text.decode("utf-8", errors="replace"), "extends"))
                        elif sub.type == "mixins":
                            for m in sub.children:
                                if m.type == "type_identifier":
                                    bases.append(
                                        (m.text.decode("utf-8", errors="replace"), "extends")
                                    )
                elif child.type == "interfaces":
                    for sub in child.children:
                        if sub.type == "type_identifier":
                            bases.append((sub.text.decode("utf-8", errors="replace"), "implements"))
        elif language == "swift":
            # Swift: class Foo: Bar, Baz { ... } / extension Foo: Protocol { ... }
            # AST: inheritance_specifier > user_type > type_identifier.
            # Two-pass protocol detection is deferred; use extends for all.
            for child in node.children:
                if child.type == "inheritance_specifier":
                    for sub in child.children:
                        if sub.type == "user_type":
                            for ident in sub.children:
                                if ident.type == "type_identifier":
                                    bases.append(
                                        (
                                            ident.text.decode("utf-8", errors="replace"),
                                            "extends",
                                        )
                                    )
                                    break
        elif language == "julia":
            if node.type in ("struct_definition", "abstract_definition"):
                for child in node.children:
                    if child.type != "type_head":
                        continue
                    for sub in child.children:
                        if sub.type != "binary_expression":
                            continue
                        has_subtype_op = False
                        for op_child in sub.children:
                            if op_child.type == "operator" and op_child.text == b"<:":
                                has_subtype_op = True
                                continue
                            if not has_subtype_op:
                                continue
                            if op_child.type == "identifier":
                                bases.append(
                                    (op_child.text.decode("utf-8", errors="replace"), "extends")
                                )
                                return bases
                            if op_child.type == "parametrized_type_expression":
                                for ident in op_child.children:
                                    if ident.type == "identifier":
                                        bases.append(
                                            (
                                                ident.text.decode("utf-8", errors="replace"),
                                                "extends",
                                            )
                                        )
                                        return bases
        return bases

    def _extract_import(self, node, language: str, source: bytes) -> list[str]:
        """Extract import targets as module/path strings."""
        imports = []
        text = node.text.decode("utf-8", errors="replace").strip()

        if language == "python":
            # import x.y.z  or  from x.y import z  or  from .x import z
            if node.type == "import_from_statement":
                for child in node.children:
                    if child.type == "relative_import":
                        # from .x import y  or  from ..x import y  or  from . import y
                        dots = 0
                        inner = ""
                        for sub in child.children:
                            if sub.type == "import_prefix":
                                dots = sum(1 for d in sub.children if d.type == ".")
                            elif sub.type == "dotted_name":
                                inner = sub.text.decode("utf-8", errors="replace")
                        imports.append("." * dots + inner)
                        break
                    if child.type == "dotted_name":
                        # Absolute: from x.y import z — first dotted_name is the module
                        imports.append(child.text.decode("utf-8", errors="replace"))
                        break
            else:
                for child in node.children:
                    if child.type == "dotted_name":
                        imports.append(child.text.decode("utf-8", errors="replace"))
        elif language in ("javascript", "typescript", "tsx"):
            # import ... from 'module'
            for child in node.children:
                if child.type == "string":
                    val = child.text.decode("utf-8", errors="replace").strip("'\"")
                    imports.append(val)
        elif language == "go":
            for child in node.children:
                if child.type == "import_spec_list":
                    for spec in child.children:
                        if spec.type == "import_spec":
                            for s in spec.children:
                                if s.type == "interpreted_string_literal":
                                    val = s.text.decode("utf-8", errors="replace")
                                    imports.append(val.strip('"'))
                elif child.type == "import_spec":
                    for s in child.children:
                        if s.type == "interpreted_string_literal":
                            val = s.text.decode("utf-8", errors="replace")
                            imports.append(val.strip('"'))
        elif language == "rust":
            # use crate::module::item
            imports.append(text.replace("use ", "").rstrip(";").strip())
        elif language in ("c", "cpp"):
            # #include <header> or #include "header"
            for child in node.children:
                if child.type in ("system_lib_string", "string_literal"):
                    val = child.text.decode("utf-8", errors="replace").strip('<>"')
                    imports.append(val)
        elif language in ("java", "csharp"):
            # import/using package.Class
            parts = text.split()
            if len(parts) >= 2:
                imports.append(parts[-1].rstrip(";"))
        elif language == "solidity":
            # import "path/to/file.sol" or import {Symbol} from "path"
            for child in node.children:
                if child.type == "string":
                    val = child.text.decode("utf-8", errors="replace").strip('"')
                    if val:
                        imports.append(val)
        elif language == "scala":
            parts = []
            selectors = []
            is_wildcard = False
            for child in node.children:
                if child.type == "identifier":
                    parts.append(child.text.decode("utf-8", errors="replace"))
                elif child.type == "namespace_selectors":
                    for sub in child.children:
                        if sub.type == "identifier":
                            selectors.append(sub.text.decode("utf-8", errors="replace"))
                elif child.type == "namespace_wildcard":
                    is_wildcard = True
            base = ".".join(parts)
            if selectors:
                for name in selectors:
                    imports.append(f"{base}.{name}")
            elif is_wildcard:
                imports.append(f"{base}.*")
            elif base:
                imports.append(base)
        elif language == "r":
            # library(pkg), require(pkg), source("file.R")
            func_name = self._r_call_func_name(node)
            if func_name in ("library", "require", "source"):
                for _name, value in self._r_iter_args(node):
                    if value.type == "identifier":
                        imports.append(value.text.decode("utf-8", errors="replace"))
                    elif value.type == "string":
                        val = self._r_first_string_arg(node)
                        if val:
                            imports.append(val)
                    break  # Only first argument matters
        elif language == "ruby":
            # require 'module' or require_relative 'path'
            if "require" in text:
                match = re.search(r"""['"](.*?)['"]""", text)
                if match:
                    imports.append(match.group(1))
        elif language == "dart":
            # import 'dart:async' or import 'package:flutter/material.dart'
            # Node structure: import_or_export > library_import > import_specification
            #                 > configurable_uri > uri > string_literal
            def _find_string_literal(n) -> Optional[str]:
                if n.type == "string_literal":
                    return n.text.decode("utf-8", errors="replace").strip("'\"")
                for c in n.children:
                    result = _find_string_literal(c)
                    if result is not None:
                        return result
                return None

            val = _find_string_literal(node)
            if val:
                imports.append(val)
        elif language == "julia":
            for child in node.children:
                if child.type == "identifier":
                    imports.append(child.text.decode("utf-8", errors="replace"))
                elif child.type == "selected_import":
                    module_name = None
                    seen_colon = False
                    for sub in child.children:
                        if sub.type == ":":
                            seen_colon = True
                            continue
                        if not seen_colon:
                            if sub.type == "identifier":
                                module_name = sub.text.decode("utf-8", errors="replace")
                        elif sub.type == "identifier" and module_name:
                            imported = sub.text.decode("utf-8", errors="replace")
                            imports.append(f"{module_name}.{imported}")
        elif language == "gdscript":
            for child in node.children:
                if child.type == "type":
                    txt = child.text.decode("utf-8", errors="replace").strip()
                    if txt:
                        imports.append(txt)
                elif child.type == "string":
                    val = child.text.decode("utf-8", errors="replace").strip("'\"")
                    if val:
                        imports.append(val)
                elif child.type == "identifier":
                    txt = child.text.decode("utf-8", errors="replace")
                    if txt and txt != "extends":
                        imports.append(txt)
        else:
            # Fallback: just record the text
            imports.append(text)

        return imports

    def _get_call_name(self, node, language: str, source: bytes) -> Optional[str]:
        """Extract the function/method name being called."""
        if not node.children:
            return None

        first = node.children[0]

        if language == "julia" and node.type == "macrocall_expression":
            for child in node.children:
                if child.type == "macro_identifier":
                    for sub in child.children:
                        if sub.type == "identifier":
                            raw = sub.text.decode("utf-8", errors="replace")
                            return f"@{raw}"
                    return None
            return None

        if language == "php":

            def _normalize_php_name(text: str) -> str:
                # PHP global/function names can be prefixed with '\\'.
                return text.lstrip("\\")

            if node.type == "function_call_expression":
                for child in node.children:
                    if child.type in ("name", "qualified_name"):
                        raw = child.text.decode("utf-8", errors="replace")
                        return _normalize_php_name(raw)
                return None

            if node.type in (
                "member_call_expression",
                "nullsafe_member_call_expression",
            ):
                for child in reversed(node.children):
                    if child.type == "name":
                        return child.text.decode("utf-8", errors="replace")
                return None

            if node.type == "scoped_call_expression":
                parts = []
                for child in node.children:
                    if child.type in ("name", "qualified_name"):
                        raw = child.text.decode("utf-8", errors="replace")
                        parts.append(_normalize_php_name(raw))
                if len(parts) >= 2:
                    return f"{parts[0]}::{parts[-1]}"
                if parts:
                    return parts[0]
                return None

        # Scala: instance_expression (new Foo(...)) – extract the type name
        if node.type == "instance_expression":
            for child in node.children:
                if child.type in ("type_identifier", "identifier"):
                    return child.text.decode("utf-8", errors="replace")
            return None

        # Objective-C: [receiver method:arg] — the method name is the
        # SECOND identifier-like child (the first is the receiver). For
        # multi-part selectors like `[obj add:a to:b]` we keep the first
        # part (`add`) as the call name; later parts are keyword arguments.
        if language == "objc" and node.type == "message_expression":
            receiver_skipped = False
            for child in node.children:
                if child.type in ("[", "]"):
                    continue
                if not receiver_skipped:
                    # First non-bracket child is the receiver (identifier,
                    # message_expression for chained calls, etc.)
                    receiver_skipped = True
                    continue
                if child.type == "identifier":
                    return child.text.decode("utf-8", errors="replace")
            return None

        # Bash: `command` node's first child is the command name.
        if language == "bash" and node.type == "command":
            for child in node.children:
                if child.type == "command_name":
                    # command_name wraps a word — get its text
                    txt = child.text.decode("utf-8", errors="replace").strip()
                    return txt or None
            return None

        if language == "gdscript" and node.type == "attribute":
            for child in reversed(node.children):
                if child.type == "attribute_call":
                    for sub in child.children:
                        if sub.type == "identifier":
                            return sub.text.decode("utf-8", errors="replace")
                    return None

        # Solidity wraps call targets in an 'expression' node – unwrap it
        if language == "solidity" and first.type == "expression" and first.children:
            first = first.children[0]

        # Perl method_call_expression: $obj->method() — find the 'method' child
        if language == "perl" and node.type == "method_call_expression":
            for child in node.children:
                if child.type == "method":
                    return child.text.decode("utf-8", errors="replace")
            return None  # method child not found

        # Simple call: func_name(args)
        # Kotlin uses "simple_identifier" instead of "identifier".
        if first.type in ("identifier", "simple_identifier"):
            return first.text.decode("utf-8", errors="replace")

        # Perl: function_call_expression / ambiguous_function_call_expression
        if first.type == "function":
            return first.text.decode("utf-8", errors="replace")

        # Lua/Luau: dot_index_expression (obj.method) and method_index_expression
        # (obj:method) — extract the rightmost identifier as the call name.
        if language in ("lua", "luau") and first.type in (
            "dot_index_expression",
            "method_index_expression",
        ):
            for child in reversed(first.children):
                if child.type == "identifier":
                    return child.text.decode("utf-8", errors="replace")
            return None

        # Method call: obj.method(args)
        # Kotlin uses "navigation_expression" for member access (obj.method).
        member_types = (
            "attribute",
            "member_expression",
            "field_expression",
            "selector_expression",
            "navigation_expression",
        )
        if first.type in member_types:
            # Get the rightmost identifier (the method name)
            # Kotlin navigation_expression uses navigation_suffix > simple_identifier.
            for child in reversed(first.children):
                if child.type in (
                    "identifier",
                    "property_identifier",
                    "field_identifier",
                    "field_name",
                    "simple_identifier",
                ):
                    return child.text.decode("utf-8", errors="replace")
                if child.type == "navigation_suffix":
                    for sub in child.children:
                        if sub.type == "simple_identifier":
                            return sub.text.decode("utf-8", errors="replace")
            return first.text.decode("utf-8", errors="replace")

        # Scoped call (e.g., Rust path::func())
        if first.type in ("scoped_identifier", "qualified_name"):
            return first.text.decode("utf-8", errors="replace")

        # R namespace-qualified call: dplyr::filter()
        if first.type == "namespace_operator":
            return first.text.decode("utf-8", errors="replace")

        return None

    def _get_jsx_component_reference(self, node) -> Optional[tuple[Optional[str], str]]:
        """Extract ``(base_name, component_name)`` for a JSX element.

        ``base_name`` is set for member-style elements such as
        ``<UI.MarkdownMsg />`` and ``None`` for plain component tags such as
        ``<MarkdownMsg />``.
        """
        for child in node.children:
            if child.type == "identifier":
                name = child.text.decode("utf-8", errors="replace")
                if self._looks_like_component_name(name):
                    return (None, name)
                return None
            if child.type == "member_expression":
                base_name = self._get_member_expression_root_name(child)
                component_name = None
                for sub in reversed(child.children):
                    if sub.type in ("identifier", "property_identifier"):
                        component_name = sub.text.decode("utf-8", errors="replace")
                        break
                if component_name and self._looks_like_component_name(component_name):
                    return (base_name, component_name)
                for sub in reversed(child.children):
                    if sub.type in ("identifier", "property_identifier"):
                        name = sub.text.decode("utf-8", errors="replace")
                        if self._looks_like_component_name(name):
                            return (None, name)
                        return None
                text = child.text.decode("utf-8", errors="replace")
                tail = text.split(".")[-1]
                if self._looks_like_component_name(tail):
                    return (None, tail)
                return None
        return None

    def _get_member_expression_root_name(self, node) -> Optional[str]:
        """Return the leftmost identifier for a nested member expression."""
        for child in node.children:
            if child.type == "identifier":
                return child.text.decode("utf-8", errors="replace")
            if child.type == "member_expression":
                return self._get_member_expression_root_name(child)
        return None

    @staticmethod
    def _looks_like_component_name(name: str) -> bool:
        """Return True for JSX names that look like user components."""
        return bool(name) and name[0].isupper()

    # Modifier suffixes used in JS/TS test runners
    _TEST_MODIFIER_SUFFIXES = frozenset(
        {
            "only",
            "skip",
            "each",
            "todo",
            "concurrent",
            "failing",
        }
    )

    def _get_base_call_name(self, node, source: bytes) -> Optional[str]:
        """Return the base object name for member-expression calls like describe.only()."""
        if not node.children:
            return None
        first = node.children[0]
        if first.type != "member_expression":
            return None
        rightmost: Optional[str] = None
        for child in reversed(first.children):
            if child.type in ("identifier", "property_identifier"):
                rightmost = child.text.decode("utf-8", errors="replace")
                break
        if rightmost not in self._TEST_MODIFIER_SUFFIXES:
            return None
        for child in first.children:
            if child.type == "identifier":
                return child.text.decode("utf-8", errors="replace")
            if child.type == "member_expression":
                for inner in child.children:
                    if inner.type == "identifier":
                        return inner.text.decode("utf-8", errors="replace")
        return None

    # ------------------------------------------------------------------
    # R-specific helpers (moved to languages/r.py)
    # ------------------------------------------------------------------

    @staticmethod
    def _r_call_func_name(call_node) -> Optional[str]:
        from .languages import r as _r_lang

        return _r_lang._r_call_func_name(call_node)

    @staticmethod
    def _r_first_string_arg(call_node) -> Optional[str]:
        from .languages import r as _r_lang

        return _r_lang._r_first_string_arg(call_node)

    @staticmethod
    def _r_iter_args(call_node):
        from .languages import r as _r_lang

        return _r_lang._r_iter_args(call_node)

    @classmethod
    def _r_find_named_arg(cls, call_node, arg_name: str):
        from .languages import r as _r_lang

        return _r_lang._r_find_named_arg(call_node, arg_name)

    # ------------------------------------------------------------------
    # R-specific handlers (moved to languages/r.py)
    # ------------------------------------------------------------------

    def _handle_r_binary_operator(self, *args, **kwargs) -> bool:
        from .languages import r as _r_lang

        return _r_lang._handle_r_binary_operator(self, *args, **kwargs)

    def _handle_r_call(self, *args, **kwargs) -> bool:
        from .languages import r as _r_lang

        return _r_lang._handle_r_call(self, *args, **kwargs)

    def _handle_r_class_call(self, *args, **kwargs) -> bool:
        from .languages import r as _r_lang

        return _r_lang._handle_r_class_call(self, *args, **kwargs)

    def _extract_r_methods(self, *args, **kwargs) -> None:
        from .languages import r as _r_lang

        _r_lang.extract_r_methods(self, *args, **kwargs)
