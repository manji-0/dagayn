"""Pure AST shape-reading functions extracted from CodeParser.

All functions are stateless — they operate only on tree-sitter nodes,
source bytes, and language strings.  No parser instance state is required.
"""

from __future__ import annotations

import re
from typing import Optional

# ---------------------------------------------------------------------------
# Type-role resolution constants (used by _resolve_type_role and _extract_classes)
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

_MAX_TEST_DESCRIPTION_LEN = 200

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

    if language == "dart" and role == "class":
        for child in node.children:
            if child.type == "abstract" or child.text == b"abstract":
                role = "abstract_class"
                break

    is_contract = role in _CONTRACT_ROLES
    is_abstract = is_contract or role in ("abstract_class", "abstract_type")
    return role, is_abstract, is_contract


def _qualify(name: str, file_path: str, enclosing_class: Optional[str]) -> str:
    """Create a qualified name: file_path::ClassName.name or file_path::name."""
    if enclosing_class:
        return f"{file_path}::{enclosing_class}.{name}"
    return f"{file_path}::{name}"


def _get_test_description(call_node, source: bytes) -> Optional[str]:
    """Extract the first string argument from a test runner call node."""
    for child in call_node.children:
        if child.type == "arguments":
            for arg in child.children:
                if arg.type in ("string", "template_string"):
                    raw = arg.text.decode("utf-8", errors="replace")
                    stripped = raw.strip("'\"`")
                    normalized = re.sub(r"\s+", " ", stripped).strip()
                    if len(normalized) > _MAX_TEST_DESCRIPTION_LEN:
                        normalized = normalized[:_MAX_TEST_DESCRIPTION_LEN]
                    return normalized
    return None


def _get_name(node, language: str, kind: str) -> Optional[str]:
    """Extract the name from a class/function definition node."""
    if language == "dart" and node.type == "function_signature":
        for child in node.children:
            if child.type == "identifier":
                return child.text.decode("utf-8", errors="replace")
        return None
    if language == "solidity":
        if node.type == "constructor_definition":
            return "constructor"
        if node.type == "fallback_receive_definition":
            for child in node.children:
                if child.type in ("receive", "fallback"):
                    return child.text.decode("utf-8", errors="replace")
    if language in ("lua", "luau") and node.type == "function_declaration":
        for child in node.children:
            if child.type in ("dot_index_expression", "method_index_expression"):
                for sub in reversed(child.children):
                    if sub.type == "identifier":
                        return sub.text.decode("utf-8", errors="replace")
                return None
    if language == "perl":
        for child in node.children:
            if child.type == "bareword":
                return child.text.decode("utf-8", errors="replace")
            if child.type == "package" and child.text != b"package":
                return child.text.decode("utf-8", errors="replace")
    if language in ("c", "cpp", "objc") and kind == "function":
        for child in node.children:
            if child.type in ("function_declarator", "pointer_declarator"):
                result = _get_name(child, language, kind)
                if result:
                    return result

    if language == "objc" and node.type == "method_definition":
        for child in node.children:
            if child.type == "identifier":
                return child.text.decode("utf-8", errors="replace")

    if language == "bash" and node.type == "function_definition":
        for child in node.children:
            if child.type == "word":
                return child.text.decode("utf-8", errors="replace")
    if language == "go" and node.type == "method_declaration":
        for child in node.children:
            if child.type == "field_identifier":
                return child.text.decode("utf-8", errors="replace")
    if language == "java" and node.type == "method_declaration":
        for child in node.children:
            if child.type == "identifier":
                return child.text.decode("utf-8", errors="replace")
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
                                            return ident.text.decode("utf-8", errors="replace")
                                if target.type == "parametrized_type_expression":
                                    for p in target.children:
                                        if p.type == "identifier":
                                            return p.text.decode("utf-8", errors="replace")
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
                                            return p.text.decode("utf-8", errors="replace")
                                    return None
                            return None
                    for sub in child.children:
                        if sub.type == "parametrized_type_expression":
                            for p in sub.children:
                                if p.type == "identifier":
                                    return p.text.decode("utf-8", errors="replace")
                            return None
            return None
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
    if language == "go" and node.type == "type_declaration":
        for child in node.children:
            if child.type == "type_spec":
                return _get_name(child, language, kind)
    return None


def _get_go_receiver_type(node) -> Optional[str]:
    """Extract the receiver type from a Go method_declaration.

    For ``func (s *T) Foo() {...}`` returns ``"T"``.
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
        return None
    return None


def _get_params(node, language: str, source: bytes) -> Optional[str]:
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
    if language == "solidity":
        params = [
            c.text.decode("utf-8", errors="replace") for c in node.children if c.type == "parameter"
        ]
        if params:
            return f"({', '.join(params)})"
    return None


def _get_return_type(node, language: str, source: bytes) -> Optional[str]:
    """Extract return type annotation if present."""
    for child in node.children:
        if child.type in ("type", "return_type", "type_annotation", "return_type_definition"):
            return child.text.decode("utf-8", errors="replace")
    if language == "python":
        for i, child in enumerate(node.children):
            if child.type == "->" and i + 1 < len(node.children):
                return node.children[i + 1].text.decode("utf-8", errors="replace")
    return None


def _get_bases(node, language: str, source: bytes) -> list[tuple[str, str]]:
    """Extract base classes and interfaces with relationship roles.

    Returns list of (name, role) tuples where role is "extends" or "implements".
    """
    bases: list[tuple[str, str]] = []
    if language == "python":
        for child in node.children:
            if child.type == "argument_list":
                for arg in child.children:
                    if arg.type in ("identifier", "attribute"):
                        bases.append((arg.text.decode("utf-8", errors="replace"), "extends"))
    elif language == "java":
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
                                    (ident.text.decode("utf-8", errors="replace"), "implements")
                                )
    elif language in ("csharp", "kotlin"):
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
            ):
                role = "implements" if child.type in _implements_types else "extends"
                text = child.text.decode("utf-8", errors="replace")
                bases.append((text, role))
    elif language == "scala":
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
                                bases.append((ident.text.decode("utf-8", errors="replace"), role))
                                first = False
                                break
    elif language == "cpp":
        for child in node.children:
            if child.type == "base_class_clause":
                for sub in child.children:
                    if sub.type == "type_identifier":
                        bases.append((sub.text.decode("utf-8", errors="replace"), "extends"))
    elif language in ("typescript", "javascript", "tsx"):

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
        for child in node.children:
            if child.type == "inheritance_specifier":
                for sub in child.children:
                    if sub.type == "user_defined_type":
                        for ident in sub.children:
                            if ident.type == "identifier":
                                bases.append(
                                    (ident.text.decode("utf-8", errors="replace"), "extends")
                                )
    elif language == "go":
        for child in node.children:
            if child.type == "type_spec":
                for sub in child.children:
                    if sub.type in ("struct_type", "interface_type"):
                        for field_node in sub.children:
                            if field_node.type == "field_declaration_list":
                                for f in field_node.children:
                                    if f.type == "type_identifier":
                                        bases.append(
                                            (f.text.decode("utf-8", errors="replace"), "extends")
                                        )
    elif language == "dart":
        for child in node.children:
            if child.type == "superclass":
                for sub in child.children:
                    if sub.type == "type_identifier":
                        bases.append((sub.text.decode("utf-8", errors="replace"), "extends"))
                    elif sub.type == "mixins":
                        for m in sub.children:
                            if m.type == "type_identifier":
                                bases.append((m.text.decode("utf-8", errors="replace"), "extends"))
            elif child.type == "interfaces":
                for sub in child.children:
                    if sub.type == "type_identifier":
                        bases.append((sub.text.decode("utf-8", errors="replace"), "implements"))
    elif language == "swift":
        for child in node.children:
            if child.type == "inheritance_specifier":
                for sub in child.children:
                    if sub.type == "user_type":
                        for ident in sub.children:
                            if ident.type == "type_identifier":
                                bases.append(
                                    (ident.text.decode("utf-8", errors="replace"), "extends")
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
                                        (ident.text.decode("utf-8", errors="replace"), "extends")
                                    )
                                    return bases
    return bases


def _get_call_name(node, language: str, source: bytes) -> Optional[str]:
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

    if node.type == "instance_expression":
        for child in node.children:
            if child.type in ("type_identifier", "identifier"):
                return child.text.decode("utf-8", errors="replace")
        return None

    if language == "objc" and node.type == "message_expression":
        receiver_skipped = False
        for child in node.children:
            if child.type in ("[", "]"):
                continue
            if not receiver_skipped:
                receiver_skipped = True
                continue
            if child.type == "identifier":
                return child.text.decode("utf-8", errors="replace")
        return None

    if language == "bash" and node.type == "command":
        for child in node.children:
            if child.type == "command_name":
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

    if language == "solidity" and first.type == "expression" and first.children:
        first = first.children[0]

    if language == "perl" and node.type == "method_call_expression":
        for child in node.children:
            if child.type == "method":
                return child.text.decode("utf-8", errors="replace")
        return None

    if first.type in ("identifier", "simple_identifier"):
        return first.text.decode("utf-8", errors="replace")

    if first.type == "function":
        return first.text.decode("utf-8", errors="replace")

    if language in ("lua", "luau") and first.type in (
        "dot_index_expression",
        "method_index_expression",
    ):
        for child in reversed(first.children):
            if child.type == "identifier":
                return child.text.decode("utf-8", errors="replace")
        return None

    member_types = (
        "attribute",
        "member_expression",
        "field_expression",
        "selector_expression",
        "navigation_expression",
    )
    if first.type in member_types:
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

    if first.type in ("scoped_identifier", "qualified_name"):
        return first.text.decode("utf-8", errors="replace")

    if first.type == "namespace_operator":
        return first.text.decode("utf-8", errors="replace")

    return None


def _looks_like_component_name(name: str) -> bool:
    """Return True for JSX names that look like user components (start with uppercase)."""
    return bool(name) and name[0].isupper()


def _get_member_expression_root_name(node) -> Optional[str]:
    """Return the leftmost identifier for a nested member expression."""
    for child in node.children:
        if child.type == "identifier":
            return child.text.decode("utf-8", errors="replace")
        if child.type == "member_expression":
            return _get_member_expression_root_name(child)
    return None


def _get_jsx_component_reference(node) -> Optional[tuple[Optional[str], str]]:
    """Extract ``(base_name, component_name)`` for a JSX element."""
    for child in node.children:
        if child.type == "identifier":
            name = child.text.decode("utf-8", errors="replace")
            if _looks_like_component_name(name):
                return (None, name)
            return None
        if child.type == "member_expression":
            base_name = _get_member_expression_root_name(child)
            component_name = None
            for sub in reversed(child.children):
                if sub.type in ("identifier", "property_identifier"):
                    component_name = sub.text.decode("utf-8", errors="replace")
                    break
            if component_name and _looks_like_component_name(component_name):
                return (base_name, component_name)
            for sub in reversed(child.children):
                if sub.type in ("identifier", "property_identifier"):
                    name = sub.text.decode("utf-8", errors="replace")
                    if _looks_like_component_name(name):
                        return (None, name)
                    return None
            text = child.text.decode("utf-8", errors="replace")
            tail = text.split(".")[-1]
            if _looks_like_component_name(tail):
                return (None, tail)
            return None
    return None


def _get_base_call_name(node, source: bytes) -> Optional[str]:
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
    if rightmost not in _TEST_MODIFIER_SUFFIXES:
        return None
    for child in first.children:
        if child.type == "identifier":
            return child.text.decode("utf-8", errors="replace")
        if child.type == "member_expression":
            for inner in child.children:
                if inner.type == "identifier":
                    return inner.text.decode("utf-8", errors="replace")
    return None


def _extract_import(node, language: str, source: bytes) -> list[str]:
    """Extract import targets as module/path strings."""
    from .languages.r import _r_call_func_name, _r_first_string_arg, _r_iter_args

    imports = []
    text = node.text.decode("utf-8", errors="replace").strip()

    if language == "python":
        if node.type == "import_from_statement":
            for child in node.children:
                if child.type == "relative_import":
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
                    imports.append(child.text.decode("utf-8", errors="replace"))
                    break
        else:
            for child in node.children:
                if child.type == "dotted_name":
                    imports.append(child.text.decode("utf-8", errors="replace"))
    elif language in ("javascript", "typescript", "tsx"):
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
        imports.append(text.replace("use ", "").rstrip(";").strip())
    elif language in ("c", "cpp"):
        for child in node.children:
            if child.type in ("system_lib_string", "string_literal"):
                val = child.text.decode("utf-8", errors="replace").strip('<>"')
                imports.append(val)
    elif language in ("java", "csharp"):
        parts = text.split()
        if len(parts) >= 2:
            imports.append(parts[-1].rstrip(";"))
    elif language == "solidity":
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
        func_name = _r_call_func_name(node)
        if func_name in ("library", "require", "source"):
            for _name, value in _r_iter_args(node):
                if value.type == "identifier":
                    imports.append(value.text.decode("utf-8", errors="replace"))
                elif value.type == "string":
                    val = _r_first_string_arg(node)
                    if val:
                        imports.append(val)
                break
    elif language == "ruby":
        if "require" in text:
            match = re.search(r"""['"](.*?)['"]""", text)
            if match:
                imports.append(match.group(1))
    elif language == "dart":

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
        imports.append(text)

    return imports
