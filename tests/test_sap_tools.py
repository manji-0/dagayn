from dagayn.tools.sap_tools import detect_sap_violations_func


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
        lambda store, scope_kind, artifact_scope, min_distance: [
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
