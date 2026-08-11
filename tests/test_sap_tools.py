from dagayn.tools.sap_tools import compute_sap_metrics_func, detect_sap_violations_func


class _DummyStore:
    def close(self) -> None:
        pass


def test_detect_sap_violations_uses_compact_envelope(monkeypatch) -> None:
    monkeypatch.setattr(
        "dagayn.tools.sap_tools._get_store",
        lambda repo_root: (_DummyStore(), None),
    )
    monkeypatch.setattr(
        "dagayn.tools.sap_tools.find_sap_violations",
        lambda store, scope_kind, artifact_scope, dependency_profile, min_distance: [
            {
                "scope_key": "pkg.alpha",
                "display_name": "pkg.alpha",
                "distance": 0.91,
                "abstractness": 0.0,
                "instability": 0.09,
            },
            {
                "scope_key": "pkg.beta",
                "display_name": "pkg.beta",
                "distance": 0.83,
                "abstractness": 1.0,
                "instability": 0.83,
            },
        ],
    )

    result = detect_sap_violations_func(scope_kind="package", min_distance=0.5, top_n=1)

    assert result["status"] == "ok"
    assert result["artifact_scope"] == "code"
    assert result["count"] == 2
    assert result["total"] == 2
    assert result["truncated"] is True
    assert "Showing top 1 by distance." in result["summary"]
    assert "suppresses test and fixture scopes" in result["summary"]
    assert result["excluded_scope_categories"] == ["test-scope", "fixture-scope"]
    assert result["exclusion_reason"] == (
        "test and fixture scopes are retained in sap_metrics notes but omitted from sap_violations"
    )
    assert result["violations"] == [
        {
            "scope_key": "pkg.alpha",
            "display_name": "pkg.alpha",
            "distance": 0.91,
            "zone": "pain",
        }
    ]


def test_compute_sap_metrics_separates_inapplicable_by_default(monkeypatch) -> None:
    monkeypatch.setattr(
        "dagayn.tools.sap_tools._get_store",
        lambda repo_root: (_DummyStore(), None),
    )
    monkeypatch.setattr(
        "dagayn.tools.sap_tools.compute_sap_metrics",
        lambda store, scope_kind, unit_filter, artifact_scope, dependency_profile: [
            {
                "scope_key": "src",
                "display_name": "src",
                "distance": 0.8,
                "sap_applicable": True,
                "applicability_reason": "applicable",
            },
            {
                "scope_key": "docs",
                "display_name": "docs",
                "distance": 1.0,
                "sap_applicable": False,
                "applicability_reason": "no-eligible-types",
            },
        ],
    )

    result = compute_sap_metrics_func(top_n=10)

    assert result["metrics"] == [
        {
            "scope_key": "src",
            "display_name": "src",
            "distance": 0.8,
            "sap_applicable": True,
            "applicability_reason": "applicable",
        }
    ]
    assert result["inapplicable_metrics"][0]["scope_key"] == "docs"
    assert result["applicable_count"] == 1
    assert result["inapplicable_count"] == 1
    assert result["inapplicable_by_reason"] == {"no-eligible-types": 1}
    assert result["inapplicable_visibility"] == "separate_bucket"
    assert result["truncated"] is False


def test_compute_sap_metrics_reports_truncation(monkeypatch) -> None:
    monkeypatch.setattr(
        "dagayn.tools.sap_tools._get_store",
        lambda repo_root: (_DummyStore(), None),
    )
    monkeypatch.setattr(
        "dagayn.tools.sap_tools.compute_sap_metrics",
        lambda store, scope_kind, unit_filter, artifact_scope, dependency_profile: [
            {
                "scope_key": f"pkg.{index}",
                "display_name": f"pkg.{index}",
                "distance": float(index),
                "sap_applicable": True,
                "applicability_reason": "applicable",
            }
            for index in range(5)
        ],
    )

    result = compute_sap_metrics_func(top_n=2)

    assert result["truncated"] is True
    assert len(result["metrics"]) == 2
    assert "Results truncated." in result["summary"]


def test_compute_sap_metrics_verbose_includes_inapplicable(monkeypatch) -> None:
    monkeypatch.setattr(
        "dagayn.tools.sap_tools._get_store",
        lambda repo_root: (_DummyStore(), None),
    )
    monkeypatch.setattr(
        "dagayn.tools.sap_tools.compute_sap_metrics",
        lambda store, scope_kind, unit_filter, artifact_scope, dependency_profile: [
            {
                "scope_key": "docs",
                "display_name": "docs",
                "distance": 1.0,
                "sap_applicable": False,
                "applicability_reason": "no-eligible-types",
            },
        ],
    )

    result = compute_sap_metrics_func(top_n=10, detail_level="verbose")

    assert result["metrics"][0]["scope_key"] == "docs"
    assert result["inapplicable_visibility"] == "included_in_metrics"
