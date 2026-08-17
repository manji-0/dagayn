"""Function-level concern separation heuristics."""

from __future__ import annotations

from pathlib import Path
from typing import Any

type ConcernValue = Any
type ConcernPayload = dict[str, ConcernValue]

_FUNCTION_BRANCH_THRESHOLD = 12
_FUNCTION_CALL_THRESHOLD = 22
_CONCERN_SPLIT_SCORE = 0.65

_AMBIGUOUS_FUNCTION_NAMES = {
    "activate",
    "build",
    "create",
    "execute",
    "handle",
    "main",
    "process",
    "register",
    "run",
    "sync",
    "update",
}

_BOOLEAN_NAME_PREFIXES = (
    "allow_",
    "dry_",
    "enable_",
    "has_",
    "include_",
    "is_",
    "should_",
    "skip_",
    "use_",
    "with_",
)

_BOUNDARY_PATH_PARTS = (
    "/api/",
    "/cli/",
    "/commands/",
    "/handlers/",
    "/routes/",
    "/server/",
    "/tools/",
)

_COORDINATOR_NAME_PREFIXES = (
    "activate",
    "build",
    "configure",
    "dispatch",
    "handle",
    "install",
    "register",
    "run",
    "serve",
    "sync",
    "update",
)

_SIDE_EFFECT_PATTERNS: tuple[tuple[str, tuple[str, ...]], ...] = (
    (
        "filesystem_io",
        (
            ".read_text(",
            ".write_text(",
            " open(",
            "Path(",
            "std::fs",
            "fs::",
            "read_to_string",
            "write_all",
        ),
    ),
    (
        "database_io",
        (
            ".execute(",
            ".executemany(",
            ".commit(",
            ".rollback(",
            ".transaction(",
            " insert ",
            " update ",
            " delete ",
            " select ",
        ),
    ),
    (
        "network_io",
        (
            "fetch(",
            "requests.",
            "urlopen(",
            "http://",
            "https://",
            "Client::",
        ),
    ),
    (
        "process_or_environment",
        (
            "os.environ",
            "process.env",
            "std::env",
            "subprocess",
            "Command::new",
            "exec(",
            "spawn(",
        ),
    ),
    (
        "time_or_random",
        (
            "datetime(",
            "datetime.",
            "Instant::",
            "random",
            "time.",
            "uuid",
        ),
    ),
    (
        "logging_or_console",
        (
            "console.",
            "eprintln!",
            "logger.",
            "logging.",
            "println!",
            "tracing::",
        ),
    ),
)


def branch_count(source_lines: list[str]) -> int:
    """Count lightweight branch and boolean-composition tokens."""

    branch_tokens = (
        " if ",
        " elif ",
        " else ",
        " for ",
        " while ",
        " match ",
        " case ",
        " switch ",
        " catch ",
        " except ",
        "&&",
        "||",
        "?",
    )
    count = 0
    for line in source_lines:
        stripped = f" {line.strip()} "
        if not stripped:
            continue
        count += sum(1 for token in branch_tokens if token in stripped)
    return count


def comment_line_count(source_lines: list[str]) -> int:
    """Count comment-like lines in a source span."""

    count = 0
    in_block = False
    for line in source_lines:
        stripped = line.strip()
        if not stripped:
            continue
        if in_block:
            count += 1
            if "*/" in stripped:
                in_block = False
            continue
        if stripped.startswith(("#", "//", "///", "//!")):
            count += 1
            continue
        if stripped.startswith(('"""', "'''")):
            count += 1
            continue
        if stripped.startswith("/*"):
            count += 1
            in_block = "*/" not in stripped
    return count


def _parameter_names(params: str | None) -> list[str]:
    if not params:
        return []
    text = params.strip()
    if text.startswith("(") and text.endswith(")"):
        text = text[1:-1]
    names: list[str] = []
    for raw_part in text.split(","):
        part = raw_part.strip()
        if not part:
            continue
        part = part.split("=", 1)[0].strip()
        part = part.split(":", 1)[0].strip()
        part = part.replace("&", "").replace("mut ", "").strip()
        if part in {"self", "cls"}:
            continue
        if " " in part:
            part = part.rsplit(" ", 1)[-1]
        if part:
            names.append(part)
    return names


def _callee_scope(target_qualified: str) -> str | None:
    if not target_qualified or target_qualified.startswith("<"):
        return None
    file_path = target_qualified.split("::", 1)[0]
    parent = Path(file_path).parent.as_posix()
    return parent or "."


def _side_effect_reason_codes(
    source_lines: list[str],
    outgoing_call_targets: list[str],
) -> list[str]:
    haystack = "\n".join(source_lines).lower()
    target_text = "\n".join(outgoing_call_targets).lower()
    reasons: list[str] = []
    for reason, patterns in _SIDE_EFFECT_PATTERNS:
        if any(
            pattern.lower() in haystack or pattern.lower() in target_text for pattern in patterns
        ):
            reasons.append(reason)
    return reasons


def _function_role(
    *,
    name: str,
    file_path: str,
    branch_count_value: int,
    outgoing_call_count: int,
    side_effect_count: int,
    context_pressure: float,
) -> str:
    normalized_file_path = file_path.replace("\\", "/")
    normalized_path = f"/{normalized_file_path}"
    normalized_name = name.lower()
    if "/tests/" in normalized_path or "/__tests__/" in normalized_path:
        return "test_helper"
    if any(part in normalized_path for part in _BOUNDARY_PATH_PARTS):
        return "boundary"
    if normalized_name.startswith(_COORDINATOR_NAME_PREFIXES):
        return "coordinator"
    if side_effect_count >= 2 and outgoing_call_count >= 3:
        return "boundary"
    if side_effect_count == 0 and branch_count_value <= 4 and outgoing_call_count <= 6:
        return "transformer"
    if side_effect_count == 0 and context_pressure < 0.35:
        return "pure_candidate"
    return "unknown"


def function_concern_profile(
    node: Any,
    source_lines: list[str],
    outgoing_edges: list[Any],
    *,
    node_community: dict[str, int] | None = None,
    branch_count_value: int | None = None,
    comment_line_count_value: int | None = None,
) -> ConcernPayload:
    """Return role-aware concern-separation evidence for a function.

    The profile is a refactoring lead, not a proof of a design smell. It
    intentionally reports pressure and missingness instead of declaring that a
    function violates single responsibility or purity.
    """

    if getattr(node, "kind", None) != "Function":
        return {}

    node_community = node_community or {}
    call_edges = [edge for edge in outgoing_edges if getattr(edge, "kind", None) == "CALLS"]
    outgoing_call_targets = [str(edge.target_qualified) for edge in call_edges]
    outgoing_call_count = len(call_edges)
    line_count = max(int(getattr(node, "line_end", 0)) - int(getattr(node, "line_start", 0)) + 1, 0)
    branches = branch_count_value if branch_count_value is not None else branch_count(source_lines)
    comments = (
        comment_line_count_value
        if comment_line_count_value is not None
        else comment_line_count(source_lines)
    )

    callee_communities = {
        community_id
        for target in outgoing_call_targets
        if (community_id := node_community.get(target)) is not None
    }
    callee_scopes = {
        scope for target in outgoing_call_targets if (scope := _callee_scope(target)) is not None
    }
    dynamic_or_unresolved_calls = sum(
        1 for target in outgoing_call_targets if target.startswith("<")
    )

    side_effect_reasons = _side_effect_reason_codes(source_lines, outgoing_call_targets)
    side_effect_count = len(side_effect_reasons)
    params = _parameter_names(getattr(node, "params", None))
    boolean_flag_count = sum(
        1
        for param in params
        if param.lower().startswith(_BOOLEAN_NAME_PREFIXES) or param.lower() in {"flag", "flags"}
    )
    parameter_count = len(params)
    missing_return_type = not bool(getattr(node, "return_type", None))
    ambiguous_name = str(getattr(node, "name", "")).lower() in _AMBIGUOUS_FUNCTION_NAMES

    responsibility_pressure = min(
        1.0,
        min(max(len(callee_communities) - 1, 0) / 3.0, 1.0) * 0.35
        + min(max(len(callee_scopes) - 2, 0) / 5.0, 1.0) * 0.25
        + min(branches / _FUNCTION_BRANCH_THRESHOLD, 1.0) * 0.2
        + min(outgoing_call_count / _FUNCTION_CALL_THRESHOLD, 1.0) * 0.2,
    )
    side_effect_pressure = min(side_effect_count / 4.0, 1.0)
    context_pressure = min(
        1.0,
        min(max(parameter_count - 4, 0) / 4.0, 1.0) * 0.45
        + min(boolean_flag_count / 2.0, 1.0) * 0.25
        + (0.2 if missing_return_type and line_count >= 60 else 0.0)
        + (0.1 if ambiguous_name else 0.0),
    )
    side_effect_mixed_with_decision_logic = side_effect_count > 0 and (
        branches >= _FUNCTION_BRANCH_THRESHOLD or outgoing_call_count >= _FUNCTION_CALL_THRESHOLD
    )
    score = min(
        1.0,
        responsibility_pressure * 0.45
        + side_effect_pressure * 0.25
        + context_pressure * 0.3
        + (0.1 if side_effect_mixed_with_decision_logic else 0.0),
    )

    reason_codes: list[str] = []
    if len(callee_communities) >= 3:
        reason_codes.append("many_callee_communities")
    if len(callee_scopes) >= 5:
        reason_codes.append("many_callee_scopes")
    if branches >= _FUNCTION_BRANCH_THRESHOLD:
        reason_codes.append("branch_heavy")
    if outgoing_call_count >= _FUNCTION_CALL_THRESHOLD:
        reason_codes.append("many_collaborators")
    if side_effect_count >= 2:
        reason_codes.append("side_effect_pressure")
    if side_effect_mixed_with_decision_logic:
        reason_codes.append("side_effect_mixed_with_decision_logic")
    if parameter_count >= 6 or boolean_flag_count >= 2:
        reason_codes.append("implicit_context")
    if context_pressure >= 0.5:
        reason_codes.append("low_context_clarity")

    confidence = "medium"
    missingness: list[str] = []
    if not source_lines:
        confidence = "low"
        missingness.append("source_unavailable")
    if outgoing_call_count and not node_community:
        confidence = "low"
        missingness.append("community_assignments_unavailable")

    role = _function_role(
        name=str(getattr(node, "name", "")),
        file_path=str(getattr(node, "file_path", "")),
        branch_count_value=branches,
        outgoing_call_count=outgoing_call_count,
        side_effect_count=side_effect_count,
        context_pressure=context_pressure,
    )
    action = "No concern-separation action needed from this profile alone."
    if score >= _CONCERN_SPLIT_SCORE:
        action = "Extract one cohesive decision or transformation helper before moving IO code."
    elif context_pressure >= 0.5:
        action = "Clarify parameters, return contract, or naming before broader refactoring."
    elif side_effect_pressure >= 0.5:
        action = "Isolate side effects from pure decision logic where possible."

    return {
        "role": role,
        "score": round(score, 3),
        "confidence": confidence,
        "reason_codes": reason_codes,
        "evidence": {
            "line_count": line_count,
            "branch_count": branches,
            "outgoing_call_count": outgoing_call_count,
            "callee_community_count": len(callee_communities),
            "callee_scope_count": len(callee_scopes),
            "dynamic_or_unresolved_call_count": dynamic_or_unresolved_calls,
            "side_effect_reason_codes": side_effect_reasons,
            "side_effect_count": side_effect_count,
            "parameter_count": parameter_count,
            "boolean_flag_parameter_count": boolean_flag_count,
            "comment_line_count": comments,
            "missing_return_type": missing_return_type,
            "ambiguous_name": ambiguous_name,
            "responsibility_pressure": round(responsibility_pressure, 3),
            "side_effect_pressure": round(side_effect_pressure, 3),
            "context_pressure": round(context_pressure, 3),
            "purity_likelihood": round(1.0 - side_effect_pressure, 3),
            "split_score_threshold": _CONCERN_SPLIT_SCORE,
        },
        "missingness": missingness,
        "action": action,
    }
