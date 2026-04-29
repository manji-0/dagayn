"""Module-resolution and file-scope collection functions.

All functions that touch parser instance state take ``parser`` as their first
argument so they can be called as plain module functions from core.py while
still accessing instance caches and sub-parsers.
"""

from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING, Optional

from . import node_shape
from ._base.types import EdgeInfo, NodeInfo
from ._constants import _CLASS_TYPES, _FUNCTION_TYPES, _IMPORT_TYPES
from .languages import julia as _julia_lang
from .languages import terraform as _terraform_lang

if TYPE_CHECKING:
    from .core import CodeParser

# ---------------------------------------------------------------------------
# File-scope collection
# ---------------------------------------------------------------------------


def _collect_file_scope(
    parser: "CodeParser",
    root,
    language: str,
    source: bytes,
) -> tuple[dict[str, str], set[str]]:
    """Pre-scan top-level AST to collect import mappings and defined names.

    Returns:
        (import_map, defined_names) where import_map maps imported names to
        their source module/path, and defined_names is the set of
        function/class names defined at file scope.
    """
    import_map: dict[str, str] = {}
    defined_names: set[str] = set()

    class_types = set(_CLASS_TYPES.get(language, []))
    func_types = set(_FUNCTION_TYPES.get(language, []))
    import_types = set(_IMPORT_TYPES.get(language, []))

    decorator_wrappers = {"decorated_definition", "decorator"}

    for child in root.children:
        node_type = child.type

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
                name = _julia_lang._julia_short_func_name(lhs)
                if name:
                    defined_names.add(name)
                    continue

        if language == "terraform":
            name = _terraform_lang._get_terraform_defined_name(target)
            if name:
                defined_names.add(name)
                continue

        if target_type in func_types or target_type in class_types:
            name = node_shape._get_name(
                target, language, "class" if target_type in class_types else "function"
            )
            if name:
                defined_names.add(name)
                continue

        if language in ("javascript", "typescript", "tsx") and node_type == "export_statement":
            _collect_js_exported_local_names(child, defined_names)

        if node_type in import_types:
            _collect_import_names(parser, child, language, source, import_map)

    return import_map, defined_names


def _collect_js_exported_local_names(
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
    parser: "CodeParser",
    node,
    language: str,
    source: bytes,
    import_map: dict[str, str],
) -> None:
    """Extract imported names and their source modules into import_map."""
    if language == "python":
        if node.type == "import_from_statement":
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
                        names = [
                            sub.text.decode("utf-8", errors="replace")
                            for sub in child.children
                            if sub.type in ("identifier", "dotted_name")
                        ]
                        if names:
                            import_map[names[-1]] = module

    elif language in ("javascript", "typescript", "tsx"):
        module = None
        for child in node.children:
            if child.type == "string":
                module = child.text.decode("utf-8", errors="replace").strip("'\"")
        if module:
            for child in node.children:
                if child.type == "import_clause":
                    _collect_js_import_names(child, module, import_map)
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
    clause_node,
    module: str,
    import_map: dict[str, str],
) -> None:
    """Walk JS/TS import_clause to extract named and default imports."""
    for child in clause_node.children:
        if child.type == "identifier":
            import_map[child.text.decode("utf-8", errors="replace")] = module
        elif child.type == "namespace_import":
            for sub in child.children:
                if sub.type == "identifier":
                    import_map[sub.text.decode("utf-8", errors="replace")] = module
                    break
        elif child.type == "named_imports":
            for spec in child.children:
                if spec.type == "import_specifier":
                    names = [
                        s.text.decode("utf-8", errors="replace")
                        for s in spec.children
                        if s.type in ("identifier", "property_identifier")
                    ]
                    if names:
                        import_map[names[-1]] = module


# ---------------------------------------------------------------------------
# Module-to-file resolution
# ---------------------------------------------------------------------------


def _resolve_module_to_file(
    parser: "CodeParser",
    module: str,
    file_path: str,
    language: str,
) -> Optional[str]:
    """Resolve a module/import path to an absolute file path.

    Uses parser._module_file_cache to avoid repeated filesystem lookups.
    """
    caller_dir = str(Path(file_path).parent)
    cache_key = f"{language}:{caller_dir}:{module}"
    if cache_key in parser._module_file_cache:
        return parser._module_file_cache[cache_key]

    resolved = _do_resolve_module(parser, module, file_path, language)
    if len(parser._module_file_cache) >= parser._MODULE_CACHE_MAX:
        parser._module_file_cache.clear()
    parser._module_file_cache[cache_key] = resolved
    return resolved


def _do_resolve_module(
    parser: "CodeParser",
    module: str,
    file_path: str,
    language: str,
) -> Optional[str]:
    """Language-aware module-to-file resolution."""
    caller_dir = Path(file_path).parent

    if language == "bash":
        try:
            target = (caller_dir / module).resolve()
            if target.is_file():
                return str(target)
        except (OSError, ValueError):
            pass
        return None

    if language == "python":
        if module.startswith("."):
            leading_dots = len(module) - len(module.lstrip("."))
            remainder = module[leading_dots:]
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
            base = caller_dir / module
            extensions = [".ts", ".tsx", ".js", ".jsx", ".vue"]
            if base.is_file():
                return str(base.resolve())
            for ext in extensions:
                target = base.with_suffix(ext)
                if target.is_file():
                    return str(target.resolve())
            if base.is_dir():
                for ext in extensions:
                    target = base / f"index{ext}"
                    if target.is_file():
                        return str(target.resolve())
        else:
            resolved = parser._tsconfig_resolver.resolve_alias(module, file_path)
            if resolved:
                return resolved

    elif language == "dart":
        if module.startswith("."):
            base = caller_dir / module
            if base.is_file():
                return str(base.resolve())
            target = base.with_suffix(".dart")
            if target.is_file():
                return str(target.resolve())
        elif module.startswith("package:"):
            try:
                uri_body = module[len("package:"):]
                pkg_name, _, sub_path = uri_body.partition("/")
                if not sub_path:
                    return None
                pubspec_root = _find_dart_pubspec_root(parser, caller_dir, pkg_name)
                if pubspec_root is not None:
                    target = pubspec_root / "lib" / sub_path
                    if target.is_file():
                        return str(target.resolve())
            except (OSError, ValueError):
                return None

    elif language == "java":
        if module.endswith(".*"):
            return None
        rel_path = module.replace(".", "/") + ".java"
        current = caller_dir
        while True:
            target = current / rel_path
            if target.is_file():
                return str(target.resolve())
            if current == current.parent:
                break
            current = current.parent
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
    parser: "CodeParser",
    start: Path,
    pkg_name: str,
) -> Optional[Path]:
    from .languages import dart as _dart_lang

    return _dart_lang.find_dart_pubspec_root(parser, start, pkg_name)


# ---------------------------------------------------------------------------
# Symbol resolution
# ---------------------------------------------------------------------------


def _resolve_call_target(
    parser: "CodeParser",
    call_name: str,
    file_path: str,
    language: str,
    import_map: dict[str, str],
    defined_names: set[str],
) -> str:
    """Resolve a bare call name to a qualified target, with fallback."""
    if call_name in defined_names:
        return node_shape._qualify(call_name, file_path, None)
    if call_name in import_map:
        resolved = _resolve_imported_symbol(
            parser,
            call_name,
            import_map[call_name],
            file_path,
            language,
        )
        if resolved:
            return resolved
    return call_name


def _resolve_imported_symbol(
    parser: "CodeParser",
    symbol_name: str,
    module: str,
    file_path: str,
    language: str,
) -> Optional[str]:
    """Resolve an imported symbol to its defining qualified name when possible."""
    resolved = _resolve_module_to_file(parser, module, file_path, language)
    if not resolved:
        return None

    export_target = _resolve_exported_symbol(parser, resolved, symbol_name)
    if export_target:
        return export_target
    return node_shape._qualify(symbol_name, resolved, None)


def _resolve_exported_symbol(
    parser: "CodeParser",
    module_file: str,
    symbol_name: str,
    seen: Optional[set[tuple[str, str]]] = None,
) -> Optional[str]:
    """Resolve a JS/TS symbol through common re-export/barrel patterns."""
    cache_key = f"{module_file}::{symbol_name}"
    if cache_key in parser._export_symbol_cache:
        return parser._export_symbol_cache[cache_key]

    key = (module_file, symbol_name)
    if seen is None:
        seen = set()
    if key in seen:
        return None
    seen.add(key)

    path = Path(module_file)
    language = parser.detect_language(path)
    if language not in ("javascript", "typescript", "tsx", "vue"):
        return None

    try:
        source = path.read_bytes()
    except (OSError, PermissionError):
        return None

    ts_parser = parser._get_parser(language)
    if not ts_parser:
        return None

    tree = ts_parser.parse(source)

    import_map, defined_names = _collect_file_scope(
        parser,
        tree.root_node,
        language,
        source,
    )
    if symbol_name in defined_names:
        result = node_shape._qualify(symbol_name, module_file, None)
        parser._export_symbol_cache[cache_key] = result
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
                    resolved_module = _resolve_module_to_file(
                        parser,
                        target_module,
                        module_file,
                        language,
                    )
                    if resolved_module:
                        result = _resolve_exported_symbol(
                            parser,
                            resolved_module,
                            original_name,
                            seen,
                        ) or node_shape._qualify(original_name, resolved_module, None)
                        parser._export_symbol_cache[cache_key] = result
                        return result
                result = node_shape._qualify(original_name, module_file, None)
                parser._export_symbol_cache[cache_key] = result
                return result

        if has_star_export and target_module:
            resolved_module = _resolve_module_to_file(
                parser,
                target_module,
                module_file,
                language,
            )
            if resolved_module:
                result = _resolve_exported_symbol(
                    parser,
                    resolved_module,
                    symbol_name,
                    seen,
                )
                if result:
                    parser._export_symbol_cache[cache_key] = result
                    return result

    parser._export_symbol_cache[cache_key] = None
    return None


# ---------------------------------------------------------------------------
# Post-parse call target resolution
# ---------------------------------------------------------------------------


def _resolve_call_targets(
    parser: "CodeParser",
    nodes: list[NodeInfo],
    edges: list[EdgeInfo],
    file_path: str,
) -> list[EdgeInfo]:
    """Resolve bare call targets to qualified names using same-file definitions.

    After parsing, CALLS edges store bare function names as targets.  This
    function builds a symbol table from the parsed nodes and qualifies any bare
    target that matches a local definition.
    """
    symbols: dict[str, str] = {}
    for node in nodes:
        if node.kind in ("Function", "Class", "Type", "Test"):
            bare = node.name
            qualified = node_shape._qualify(bare, file_path, node.parent_name)
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
