"""Dependency edge profiles for architecture and changeability metrics."""

from __future__ import annotations

from typing import Any, Literal, cast

DependencyProfile = Literal[
    "strict_static",
    "implementation",
    "infra_dataflow",
    "artifact_trace",
]

STRICT_STATIC_EDGE_KINDS: frozenset[str] = frozenset(
    {"IMPORTS_FROM", "DEPENDS_ON", "INHERITS", "IMPLEMENTS"}
)

DEPENDENCY_PROFILE_EDGE_KINDS: dict[str, frozenset[str]] = {
    "strict_static": STRICT_STATIC_EDGE_KINDS,
    "implementation": STRICT_STATIC_EDGE_KINDS | {"CALLS"},
    "infra_dataflow": STRICT_STATIC_EDGE_KINDS | {"REFERENCES"},
    "artifact_trace": STRICT_STATIC_EDGE_KINDS | {"CROSS_ARTIFACT"},
}


def validate_dependency_profile(profile: str = "strict_static") -> DependencyProfile:
    """Return a validated dependency profile name or raise a clear error."""
    if profile in DEPENDENCY_PROFILE_EDGE_KINDS:
        return cast(DependencyProfile, profile)
    allowed = ", ".join(sorted(DEPENDENCY_PROFILE_EDGE_KINDS))
    raise ValueError(f"Unknown dependency_profile {profile!r}. Expected one of: {allowed}.")


def dependency_edge_kinds(profile: str = "strict_static") -> frozenset[str]:
    """Return edge kinds considered by a named architecture dependency profile."""
    return DEPENDENCY_PROFILE_EDGE_KINDS[validate_dependency_profile(profile)]


def edge_matches_dependency_profile(
    edge: Any,
    profile: str = "strict_static",
) -> bool:
    """Return whether *edge* belongs to a named architecture dependency profile."""
    kind = getattr(edge, "kind", None)
    if kind not in dependency_edge_kinds(profile):
        return False
    if kind != "CROSS_ARTIFACT":
        return True

    extra = getattr(edge, "extra", None)
    extra = extra if isinstance(extra, dict) else {}
    role = extra.get("relationship_role")
    target = str(getattr(edge, "target_qualified", ""))
    tier = str(getattr(edge, "confidence_tier", "") or extra.get("confidence_tier", "")).upper()
    if role == "describes_symbol" and target.startswith("<unresolved:") and tier == "LOW":
        return False
    return tier in {"EXACT", "HIGH", "EXTRACTED"}
