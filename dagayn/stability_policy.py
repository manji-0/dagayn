"""Shared stability policy signals for review, architecture, and refactor tools."""

from __future__ import annotations

from typing import Any

from ._scope import node_file_to_scope_key

type StabilityValue = Any
type StabilityProfile = dict[str, StabilityValue]
type StabilityProfiles = dict[str, StabilityProfile]

STABLE_INSTABILITY_MAX = 0.35
SHOULD_BE_STABLE_CA_MIN = 3
STABLE_TEST_DENSITY_TARGET = 0.8
STABLE_DOC_DENSITY_TARGET = 0.5
DEFAULT_TEST_DENSITY_TARGET = 0.5
DEFAULT_DOC_DENSITY_TARGET = 0.25


def scope_key_for_file(file_path: str | None) -> str | None:
    """Return the package-level policy scope for a file path."""
    if not file_path:
        return None
    return node_file_to_scope_key(file_path, "package")


def component_stability_profiles(store: Any, snapshot: Any | None = None) -> StabilityProfiles:
    """Return package-level stability expectations from SDP/SAP metrics.

    ``snapshot`` may supply the shared node/edge lists so the underlying SDP
    and SAP metric computations skip fresh table reads when several analyses
    run together.
    """
    try:
        from .architecture import compute_sdp_metrics
        from .sap import compute_sap_metrics

        sdp_metrics = compute_sdp_metrics(
            store, granularity="package", artifact_scope="code", snapshot=snapshot
        )
        sap_metrics = compute_sap_metrics(
            store, scope_kind="package", artifact_scope="code", snapshot=snapshot
        )
    except Exception:  # pragma: no cover - defensive for backend parity drift
        return {}

    profiles: StabilityProfiles = {}
    thresholds = stability_thresholds()
    for metric in sdp_metrics:
        scope_key = str(metric.get("name", ""))
        if not scope_key:
            continue
        ca = int(metric.get("ca", 0) or 0)
        ce = int(metric.get("ce", 0) or 0)
        instability = float(metric.get("instability", 0.0) or 0.0)
        reason_codes: list[str] = []
        if ca + ce > 0 and instability <= STABLE_INSTABILITY_MAX:
            reason_codes.append("observed_stable_component")
        if ca >= SHOULD_BE_STABLE_CA_MIN or (ca >= 2 and ca > ce):
            reason_codes.append("high_afferent_coupling_should_be_stable")
        profiles[scope_key] = {
            "scope_key": scope_key,
            "ca": ca,
            "ce": ce,
            "instability": round(instability, 4),
            "stable": "observed_stable_component" in reason_codes,
            "should_be_stable": "high_afferent_coupling_should_be_stable" in reason_codes,
            "reason_codes": reason_codes,
            "thresholds": thresholds,
            "expected_test_density": (
                STABLE_TEST_DENSITY_TARGET if reason_codes else DEFAULT_TEST_DENSITY_TARGET
            ),
            "test_density_metric": "direct_test_density",
            "supplemental_test_density_metrics": [
                "heuristic_test_density",
                "transitive_test_density",
            ],
            "expected_doc_density": (
                STABLE_DOC_DENSITY_TARGET if reason_codes else DEFAULT_DOC_DENSITY_TARGET
            ),
        }

    for metric in sap_metrics:
        scope_key = str(metric.get("scope_key", ""))
        if not scope_key:
            continue
        profile = profiles.setdefault(
            scope_key,
            {
                "scope_key": scope_key,
                "ca": int(metric.get("ca", 0) or 0),
                "ce": int(metric.get("ce", 0) or 0),
                "instability": float(metric.get("instability", 0.0) or 0.0),
                "stable": False,
                "should_be_stable": False,
                "reason_codes": [],
                "thresholds": thresholds,
                "expected_test_density": DEFAULT_TEST_DENSITY_TARGET,
                "test_density_metric": "direct_test_density",
                "supplemental_test_density_metrics": [
                    "heuristic_test_density",
                    "transitive_test_density",
                ],
                "expected_doc_density": DEFAULT_DOC_DENSITY_TARGET,
            },
        )
        profile["abstractness"] = metric.get("abstractness")
        profile["sap_distance"] = metric.get("distance")
        profile["sap_notes"] = metric.get("notes", [])
        profile["sap_applicable"] = metric.get("sap_applicable", True)
        profile["sap_applicability_reason"] = metric.get("applicability_reason", "applicable")
        if not profile["sap_applicable"]:
            continue
        distance = float(metric.get("distance", 0.0) or 0.0)
        instability = float(profile.get("instability", 0.0) or 0.0)
        if distance >= 0.5 and instability <= STABLE_INSTABILITY_MAX:
            if "stable_concrete_pressure" not in profile["reason_codes"]:
                profile["reason_codes"].append("stable_concrete_pressure")
            profile["should_be_stable"] = True
            profile["expected_test_density"] = STABLE_TEST_DENSITY_TARGET
            profile["expected_doc_density"] = STABLE_DOC_DENSITY_TARGET

    return profiles


def stability_thresholds() -> StabilityProfile:
    """Expose the thresholds used by the shared stability policy."""
    return {
        "instability_max": STABLE_INSTABILITY_MAX,
        "afferent_coupling_should_be_stable_min": SHOULD_BE_STABLE_CA_MIN,
        "stable_expected_test_density": STABLE_TEST_DENSITY_TARGET,
        "stable_expected_doc_density": STABLE_DOC_DENSITY_TARGET,
        "default_expected_test_density": DEFAULT_TEST_DENSITY_TARGET,
        "default_expected_doc_density": DEFAULT_DOC_DENSITY_TARGET,
    }


def stability_policy_summary(
    profiles: StabilityProfiles,
    *,
    limit: int = 5,
) -> StabilityProfile:
    """Summarize shared stability policy outcomes for user-facing tools."""
    stable = [item for item in profiles.values() if item.get("stable")]
    should = [item for item in profiles.values() if item.get("should_be_stable")]
    pressure = [
        item
        for item in profiles.values()
        if "stable_concrete_pressure" in item.get("reason_codes", [])
    ]
    examples = sorted(
        [*stable, *should],
        key=lambda item: (
            float(item.get("instability", 1.0) or 1.0),
            -int(item.get("ca", 0) or 0),
            str(item.get("scope_key", "")),
        ),
    )
    return {
        "thresholds": stability_thresholds(),
        "counts": {
            "profiled_components": len(profiles),
            "stable_components": len(stable),
            "should_be_stable_components": len(should),
            "stable_concrete_pressure_components": len(pressure),
        },
        "reason_codes": sorted(
            {code for item in profiles.values() for code in item.get("reason_codes", [])}
        ),
        "top_examples": [
            {
                "scope_key": item.get("scope_key"),
                "instability": item.get("instability"),
                "ca": item.get("ca"),
                "ce": item.get("ce"),
                "reason_codes": item.get("reason_codes", []),
            }
            for item in examples[:limit]
        ],
    }
