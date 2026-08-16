"""Golden tests for shared stability policy reason codes."""

from __future__ import annotations

from dagayn.stability_policy import component_stability_profiles


def test_component_stability_profiles_reason_codes_and_density_policy(monkeypatch):
    monkeypatch.setattr(
        "dagayn.architecture.compute_sdp_metrics",
        lambda store, granularity, artifact_scope, **kwargs: [
            {"name": "core", "ca": 4, "ce": 1, "instability": 0.2},
            {"name": "service", "ca": 2, "ce": 1, "instability": 0.4},
        ],
    )
    monkeypatch.setattr(
        "dagayn.sap.compute_sap_metrics",
        lambda store, scope_kind, artifact_scope, **kwargs: [
            {"scope_key": "core", "abstractness": 0.0, "distance": 0.8},
            {"scope_key": "service", "abstractness": 0.0, "distance": 0.1},
            {
                "scope_key": "scripts",
                "abstractness": 0.0,
                "distance": 1.0,
                "sap_applicable": False,
                "applicability_reason": "no-eligible-types",
            },
        ],
    )

    profiles = component_stability_profiles(object())

    assert profiles["core"]["reason_codes"] == [
        "observed_stable_component",
        "high_afferent_coupling_should_be_stable",
        "stable_concrete_pressure",
    ]
    assert profiles["core"]["stable"] is True
    assert profiles["core"]["should_be_stable"] is True
    assert profiles["core"]["test_density_metric"] == "direct_test_density"
    assert profiles["core"]["supplemental_test_density_metrics"] == [
        "heuristic_test_density",
        "transitive_test_density",
    ]
    assert profiles["service"]["reason_codes"] == ["high_afferent_coupling_should_be_stable"]
    assert "stable_concrete_pressure" not in profiles["scripts"]["reason_codes"]
    assert profiles["scripts"]["sap_applicable"] is False
