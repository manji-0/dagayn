"""Tree-sitter based multi-language code parser.

Extracts structural nodes (classes, functions, imports, types) and edges
(calls, inheritance, contains) from source files.
"""

from __future__ import annotations

import logging
from pathlib import Path
from typing import Optional

from ..tsconfig_resolver import TsconfigResolver
from . import bridges, dispatch, grammars, node_shape, refs, resolver
from ._base.test_detection import (
    _TEST_RUNNER_NAMES,
)
from ._base.test_detection import (
    is_test_file as _is_test_file,
)
from ._base.test_detection import (
    is_test_function as _is_test_function,
)
from ._base.types import EdgeInfo, NodeInfo
from ._constants import _CALL_TYPES, _CLASS_TYPES, _FUNCTION_TYPES, _IMPORT_TYPES
from .bridges import _BRIDGE_PATTERNS
from .languages import SPECIAL_HANDLERS as _SPECIAL_HANDLERS
from .languages import julia as _julia_lang
from .languages import markdown as _markdown_lang
from .languages import notebook as _notebook_lang
from .languages import rescript as _rescript_lang
from .languages import svelte as _svelte_lang
from .languages import vue as _vue_lang

logger = logging.getLogger(__name__)


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
                    qn = node_shape._qualify(n.name, n.file_path, n.parent_name)
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
        return resolver._resolve_call_targets(self, nodes, edges, file_path)

    _MAX_AST_DEPTH = 180  # Guard against pathologically nested source files

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
                refs._extract_jsx_component_call(
                    self,
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
            refs._extract_value_references(
                self,
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
        name = node_shape._get_name(child, language, "class")
        if not name:
            return False

        extra: dict = {}
        _type_role, _is_abstract, _is_contract = node_shape._resolve_type_role(child, language)
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
                target=node_shape._qualify(name, file_path, enclosing_class),
                file_path=file_path,
                line=child.start_point[0] + 1,
            )
        )

        # Inheritance / implementation edges
        bases = node_shape._get_bases(child, language, source)
        for base_name, base_role in bases:
            edge_kind = "IMPLEMENTS" if base_role == "implements" else "INHERITS"
            edges.append(
                EdgeInfo(
                    kind=edge_kind,
                    source=node_shape._qualify(
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
        name = node_shape._get_name(child, language, "function")
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
            receiver_type = node_shape._get_go_receiver_type(child)
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
        qualified = node_shape._qualify(name, file_path, enclosing_class)
        params = node_shape._get_params(child, language, source)
        ret_type = node_shape._get_return_type(child, language, source)

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
            node_shape._qualify(enclosing_class, file_path, None) if enclosing_class else file_path
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
            owner = _julia_lang._julia_qualified_function_owner(child)
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
            effective_call_name = node_shape._get_base_call_name(child, source) or call_name

        # Special handling: test runner calls in test files -> Test nodes
        if (
            effective_call_name
            and language in ("javascript", "typescript", "tsx")
            and _is_test_file(file_path)
            and effective_call_name in _TEST_RUNNER_NAMES
        ):
            test_desc = node_shape._get_test_description(child, source)
            line_no = child.start_point[0] + 1
            synthetic_base = (
                f"{effective_call_name}:{test_desc}" if test_desc else effective_call_name
            )
            synthetic_name = f"{synthetic_base}@L{line_no}"
            qualified = node_shape._qualify(
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
                node_shape._qualify(
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
                node_shape._qualify(enclosing_func, file_path, enclosing_class)
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
                node_shape._qualify(enclosing_func, file_path, enclosing_class)
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

    # ------------------------------------------------------------------
    # Class shims — delegate to module functions so that language modules
    # calling ``parser.<method>(...)`` continue to work unchanged.
    # ------------------------------------------------------------------

    def _qualify(self, name: str, file_path: str, enclosing_class) -> str:
        return node_shape._qualify(name, file_path, enclosing_class)

    def _get_name(self, node, language: str, kind: str):
        return node_shape._get_name(node, language, kind)

    def _get_params(self, node, language: str, source: bytes):
        return node_shape._get_params(node, language, source)

    def _get_return_type(self, node, language: str, source: bytes):
        return node_shape._get_return_type(node, language, source)

    def _get_call_name(self, node, language: str, source: bytes):
        return node_shape._get_call_name(node, language, source)

    def _extract_import(self, node, language: str, source: bytes) -> list[str]:
        return node_shape._extract_import(node, language, source)

    def _collect_file_scope(self, root, language: str, source: bytes):
        return resolver._collect_file_scope(self, root, language, source)

    def _collect_import_names(self, node, language: str, source: bytes, import_map: dict) -> None:
        resolver._collect_import_names(self, node, language, source, import_map)

    def _resolve_module_to_file(self, module: str, file_path: str, language: str):
        return resolver._resolve_module_to_file(self, module, file_path, language)

    def _resolve_call_target(
        self,
        call_name: str,
        file_path: str,
        language: str,
        import_map: dict,
        defined_names: set,
    ) -> str:
        return resolver._resolve_call_target(
            self, call_name, file_path, language, import_map, defined_names
        )

    def _resolve_imported_symbol(
        self,
        symbol_name: str,
        module: str,
        file_path: str,
        language: str,
    ):
        return resolver._resolve_imported_symbol(self, symbol_name, module, file_path, language)

