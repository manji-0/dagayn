"""Tests for the MCP server entry point.

Focused on the ``_resolve_repo_root`` helper that threads the
``serve --repo <X>`` CLI flag into every tool wrapper, and on the
set of tools that must be registered as async coroutines so the MCP
stdio event loop stays responsive during long-running operations.
"""

from __future__ import annotations

import asyncio
import inspect

import pytest

from dagayn import main as crg_main

SPLIT_ARCHITECTURE_TOOL_NAMES = {
    "get_architecture_overview_tool",
    "list_communities_tool",
    "get_community_tool",
    "get_hub_nodes_tool",
    "get_bridge_nodes_tool",
    "get_knowledge_gaps_tool",
    "get_surprising_connections_tool",
    "detect_adp_violations_tool",
    "compute_sdp_metrics_tool",
    "detect_sdp_violations_tool",
    "compute_sap_metrics_tool",
    "detect_sap_violations_tool",
}

SPLIT_REVIEW_FLOW_TOOL_NAMES = {
    "detect_changes_tool",
    "get_review_context_tool",
    "get_affected_flows_tool",
    "get_impact_radius_tool",
    "list_flows_tool",
    "get_flow_tool",
}


def _tool_names() -> set[str]:
    import asyncio

    async def _inner() -> set[str]:
        return {tool.name for tool in await crg_main.mcp.list_tools()}

    return asyncio.run(_inner())


class TestResolveRepoRoot:
    """Precedence rules for _resolve_repo_root (see #222 follow-up)."""

    @pytest.fixture(autouse=True)
    def _reset_default(self):
        """Save and restore the module-level default before/after each test."""
        original = crg_main._default_repo_root
        yield
        crg_main._default_repo_root = original

    def test_none_when_neither_is_set(self):
        crg_main._default_repo_root = None
        assert crg_main._resolve_repo_root(None) is None

    def test_empty_string_treated_as_unset(self):
        """Empty string from an MCP client should not shadow the --repo flag."""
        crg_main._default_repo_root = "/tmp/flag-repo"
        assert crg_main._resolve_repo_root("") == "/tmp/flag-repo"

    def test_flag_used_when_client_omits_repo_root(self):
        crg_main._default_repo_root = "/tmp/flag-repo"
        assert crg_main._resolve_repo_root(None) == "/tmp/flag-repo"

    def test_client_arg_wins_over_flag(self):
        crg_main._default_repo_root = "/tmp/flag-repo"
        assert crg_main._resolve_repo_root("/explicit") == "/explicit"

    def test_client_arg_used_when_no_flag(self):
        crg_main._default_repo_root = None
        assert crg_main._resolve_repo_root("/explicit") == "/explicit"


class TestResolveEmbeddingDefaults:
    """Precedence rules for MCP semantic-search embedding defaults."""

    @pytest.fixture(autouse=True)
    def _reset_defaults(self, monkeypatch):
        original_provider = crg_main._default_embedding_provider
        original_model = crg_main._default_embedding_model
        original_local = crg_main._default_local_embedding
        original_local_port = crg_main._default_local_embedding_port
        original_local_bin = crg_main._default_local_embedding_bin
        original_keep_local = crg_main._default_keep_local_embedding_server
        original_local_timeout = crg_main._default_local_embedding_timeout
        original_local_request_timeout = crg_main._default_local_embedding_request_timeout
        original_local_batch_size = crg_main._default_local_embedding_batch_size
        for key in (
            "CRG_OPENAI_API_KEY",
            "CRG_OPENAI_BASE_URL",
            "CRG_OPENAI_MODEL",
            "GOOGLE_API_KEY",
            "MINIMAX_API_KEY",
        ):
            monkeypatch.delenv(key, raising=False)
        yield
        crg_main._default_embedding_provider = original_provider
        crg_main._default_embedding_model = original_model
        crg_main._default_local_embedding = original_local
        crg_main._default_local_embedding_port = original_local_port
        crg_main._default_local_embedding_bin = original_local_bin
        crg_main._default_keep_local_embedding_server = original_keep_local
        crg_main._default_local_embedding_timeout = original_local_timeout
        crg_main._default_local_embedding_request_timeout = original_local_request_timeout
        crg_main._default_local_embedding_batch_size = original_local_batch_size

    def test_explicit_client_provider_wins(self):
        crg_main._default_embedding_provider = "openai"
        assert crg_main._resolve_embedding_provider("google") == "google"

    def test_server_default_used_when_client_omits_provider(self):
        crg_main._default_embedding_provider = "openai"
        assert crg_main._resolve_embedding_provider(None) == "openai"

    def test_server_default_model_used_when_client_omits_model(self):
        crg_main._default_embedding_model = "qwen3-embedding-0.6b-gguf-q8_0"
        assert crg_main._resolve_embedding_model(None) == "qwen3-embedding-0.6b-gguf-q8_0"

    def test_infers_openai_when_only_openai_env_is_configured(self, monkeypatch):
        monkeypatch.setenv("CRG_OPENAI_API_KEY", "k")
        monkeypatch.setenv("CRG_OPENAI_BASE_URL", "http://127.0.0.1:18080/v1")
        monkeypatch.setenv("CRG_OPENAI_MODEL", "m")
        assert crg_main._infer_remote_embedding_provider_from_env() == "openai"

    def test_does_not_guess_when_multiple_remote_envs_are_configured(self, monkeypatch):
        monkeypatch.setenv("CRG_OPENAI_API_KEY", "k")
        monkeypatch.setenv("CRG_OPENAI_BASE_URL", "https://example.test/v1")
        monkeypatch.setenv("CRG_OPENAI_MODEL", "m")
        monkeypatch.setenv("GOOGLE_API_KEY", "g")
        assert crg_main._infer_remote_embedding_provider_from_env() is None

    def test_semantic_search_tool_applies_server_defaults(self, monkeypatch):
        calls: list[dict] = []

        def fake_tool(name):
            assert name == "semantic_search_nodes"

            def fake_semantic_search_nodes(**kwargs):
                calls.append(kwargs)
                return {"status": "ok", "results": []}

            return fake_semantic_search_nodes

        monkeypatch.setattr(crg_main, "_tool", fake_tool)
        crg_main._default_embedding_provider = "openai"
        crg_main._default_embedding_model = "qwen3"

        crg_main.semantic_search_nodes_tool(query="embedding search")

        assert calls[0]["provider"] == "openai"
        assert calls[0]["model"] == "qwen3"

    def test_cross_repo_search_tool_applies_server_defaults(self, monkeypatch):
        calls: list[dict] = []

        def fake_tool(name):
            assert name == "cross_repo_search_func"

            def fake_cross_repo_search_func(**kwargs):
                calls.append(kwargs)
                return {"status": "ok", "results": []}

            return fake_cross_repo_search_func

        monkeypatch.setattr(crg_main, "_tool", fake_tool)
        crg_main._default_embedding_provider = "openai"
        crg_main._default_embedding_model = "qwen3"

        crg_main.cross_repo_search_tool(query="embedding search")

        assert calls[0]["provider"] == "openai"
        assert calls[0]["model"] == "qwen3"

    def test_traverse_graph_tool_applies_server_defaults(self, monkeypatch):
        calls: list[dict] = []

        def fake_tool(name):
            assert name == "traverse_graph_func"

            def fake_traverse_graph_func(**kwargs):
                calls.append(kwargs)
                return {"status": "ok", "traversal": []}

            return fake_traverse_graph_func

        monkeypatch.setattr(crg_main, "_tool", fake_tool)
        crg_main._default_embedding_provider = "openai"
        crg_main._default_embedding_model = "qwen3"

        crg_main.traverse_graph_tool(query="embedding search")

        assert calls[0]["provider"] == "openai"
        assert calls[0]["model"] == "qwen3"

    def test_build_update_tool_applies_server_local_embedding_default(self, monkeypatch):
        calls: list[dict] = []

        def fake_tool(name):
            assert name == "build_or_update_graph"

            def fake_build_or_update_graph(**kwargs):
                calls.append(kwargs)
                return {"status": "ok"}

            return fake_build_or_update_graph

        monkeypatch.setattr(crg_main, "_tool", fake_tool)
        crg_main._default_local_embedding = "low"
        crg_main._default_local_embedding_port = 19090
        crg_main._default_local_embedding_bin = "/tmp/llama-server"
        crg_main._default_keep_local_embedding_server = True
        crg_main._default_local_embedding_timeout = 12
        crg_main._default_local_embedding_request_timeout = 17
        crg_main._default_local_embedding_batch_size = 8

        asyncio.run(crg_main.build_or_update_graph_tool(repo_root="/repo"))

        assert calls[0]["local_embedding"] == "low"
        assert calls[0]["local_embedding_port"] == 19090
        assert calls[0]["local_embedding_bin"] == "/tmp/llama-server"
        assert calls[0]["keep_local_embedding_server"] is True
        assert calls[0]["local_embedding_timeout"] == 12
        assert calls[0]["local_embedding_request_timeout"] == 17
        assert calls[0]["local_embedding_batch_size"] == 8

    def test_build_update_tool_explicit_none_overrides_server_local_embedding(self, monkeypatch):
        calls: list[dict] = []

        def fake_tool(name):
            assert name == "build_or_update_graph"

            def fake_build_or_update_graph(**kwargs):
                calls.append(kwargs)
                return {"status": "ok"}

            return fake_build_or_update_graph

        monkeypatch.setattr(crg_main, "_tool", fake_tool)
        crg_main._default_local_embedding = "low"

        asyncio.run(crg_main.build_or_update_graph_tool(repo_root="/repo", local_embedding="none"))

        assert calls[0]["local_embedding"] == "none"


class TestServeMainTransport:
    """``main()`` wires FastMCP to stdio or Streamable HTTP."""

    @pytest.fixture(autouse=True)
    def _restore_tools_and_env(self, monkeypatch):
        original = crg_main._snapshot_components()
        original_provider = crg_main._default_embedding_provider
        original_model = crg_main._default_embedding_model
        monkeypatch.delenv("CRG_TOOLS", raising=False)
        yield
        crg_main._restore_components(original)
        crg_main._default_embedding_provider = original_provider
        crg_main._default_embedding_model = original_model

    def test_stdio_calls_mcp_run_stdio(self, monkeypatch):
        calls: list[dict] = []

        def fake_run(**kwargs):
            calls.append(kwargs)

        monkeypatch.setattr(crg_main.mcp, "run", fake_run)
        crg_main.main(repo_root=None)
        assert calls == [{"transport": "stdio", "show_banner": False}]

    def test_main_records_embedding_defaults(self, monkeypatch):
        monkeypatch.setattr(crg_main.mcp, "run", lambda **_kwargs: None)
        crg_main.main(
            repo_root=None,
            embedding_provider="openai",
            embedding_model="qwen3",
            local_embedding="low",
            local_embedding_port=19090,
            local_embedding_bin="/tmp/llama-server",
            keep_local_embedding_server=True,
            local_embedding_timeout=12,
            local_embedding_request_timeout=17,
            local_embedding_batch_size=8,
        )

        assert crg_main._default_embedding_provider == "openai"
        assert crg_main._default_embedding_model == "qwen3"
        assert crg_main._default_local_embedding == "low"
        assert crg_main._default_local_embedding_port == 19090
        assert crg_main._default_local_embedding_bin == "/tmp/llama-server"
        assert crg_main._default_keep_local_embedding_server is True
        assert crg_main._default_local_embedding_timeout == 12
        assert crg_main._default_local_embedding_request_timeout == 17
        assert crg_main._default_local_embedding_batch_size == 8

    def test_http_calls_mcp_run_with_host_port(self, monkeypatch):
        calls: list[dict] = []

        def fake_run(**kwargs):
            calls.append(kwargs)

        monkeypatch.setattr(crg_main.mcp, "run", fake_run)
        crg_main.main(
            repo_root="/tmp/r",
            transport="streamable-http",
            host="127.0.0.1",
            port=5555,
        )
        assert calls == [
            {
                "transport": "streamable-http",
                "host": "127.0.0.1",
                "port": 5555,
            }
        ]

    def test_streamable_http_without_host_port_raises(self):
        with pytest.raises(ValueError, match="requires host and port"):
            crg_main.main(transport="streamable-http", host=None, port=5555)
        with pytest.raises(ValueError, match="requires host and port"):
            crg_main.main(transport="streamable-http", host="127.0.0.1", port=None)


class TestLongRunningToolsAreAsync:
    """Long-running MCP tools must be registered as coroutines so the
    asyncio event loop stays responsive while the work runs in a
    background thread via ``asyncio.to_thread``. Without this, Windows
    MCP clients hang on ``build_or_update_graph_tool`` and
    ``embed_graph_tool`` — see #46, #136.
    """

    HEAVY_TOOLS = {
        "build_or_update_graph_tool",
        "run_postprocess_tool",
        "embed_graph_tool",
        "review_tool",
        "generate_wiki_tool",
    }

    def test_heavy_tools_are_coroutines(self):
        """Regression guard for #46/#136: the 5 long-running MCP tools must
        stay ``async def`` so FastMCP can offload their blocking work via
        ``asyncio.to_thread`` and keep the stdio event loop responsive.

        The original implementation of this test went through
        ``crg_main.mcp.get_tools()``, which does not exist in the FastMCP
        2.14+ API pinned in pyproject.toml (``list_tools()`` replaces it and
        returns MCP protocol ``Tool`` objects, which do not expose the
        underlying Python function at all).  The sibling test
        ``test_heavy_tool_source_uses_to_thread`` already resolves each
        tool by ``getattr(crg_main, name)``; we do the same here so this
        guard is independent of any FastMCP internal surface.  See #239.
        """
        missing: list[str] = []
        not_async: list[str] = []

        for tool_name in self.HEAVY_TOOLS:
            fn = getattr(crg_main, tool_name, None)
            if fn is None:
                missing.append(tool_name)
                continue
            # The @mcp.tool() decorator wraps the function; FunctionTool
            # stores the underlying callable on ``.fn`` on current FastMCP
            # 2.x but we fall back to the wrapper itself for resilience.
            underlying = getattr(fn, "fn", None) or fn
            if not asyncio.iscoroutinefunction(underlying):
                not_async.append(tool_name)

        assert not missing, f"heavy tool(s) not registered at all: {missing}"
        assert not not_async, (
            f"these tools must be async but were registered as sync, "
            f"which will hang the stdio event loop on Windows: {not_async}"
        )

    def test_heavy_tool_source_uses_to_thread(self):
        """Defense in depth: the source of every heavy tool wrapper must
        literally call asyncio.to_thread so we don't accidentally turn
        a tool async without offloading the blocking work."""
        for tool_name in self.HEAVY_TOOLS:
            fn = getattr(crg_main, tool_name, None)
            assert fn is not None, f"{tool_name} not found on module"
            # The @mcp.tool() decorator wraps the original function; walk
            # through the wrapper to find the underlying source.
            underlying = getattr(fn, "fn", None) or fn
            source = inspect.getsource(underlying)
            assert "asyncio.to_thread" in source, (
                f"{tool_name} must call asyncio.to_thread to offload its "
                f"blocking work; otherwise Windows MCP clients will hang. "
                f"See #46, #136."
            )

    def test_regression_guard_does_not_depend_on_fastmcp_internals(self):
        """Regression guard for #239 bug 3: ensure the async guards above
        resolve heavy tools by module attribute lookup, NOT through a
        FastMCP internal API that may drift between releases.

        The original ``test_heavy_tools_are_coroutines`` called an API on
        the mcp instance that does not exist in ``fastmcp>=2.14.0``.  It
        died with ``AttributeError`` at runtime on every platform,
        silently disabling the async-regression guard that was supposed
        to protect #46/#136 from regressing.  This test locks in the
        module-lookup approach so the guards keep working regardless of
        internal FastMCP surface changes.
        """
        import ast as _ast

        # Every heavy tool must be reachable by plain getattr on the
        # module — that's the only API surface the guards are allowed to
        # use.  No mcp internals.
        for tool_name in self.HEAVY_TOOLS:
            fn = getattr(crg_main, tool_name, None)
            assert fn is not None, (
                f"{tool_name} must be reachable via "
                f"getattr(crg_main, tool_name) so the async guards "
                f"do not depend on any FastMCP internal API"
            )

        # And the guards themselves must not reference renamed/removed
        # APIs on the mcp instance.  We check the parsed AST of the
        # function bodies (not the docstrings) so an explanatory comment
        # mentioning an old API name doesn't trip this guard.
        forbidden_mcp_attrs = {
            "get_tools",
            "_tools",
            "tool_manager",
            "_tool_manager",
        }
        for guard_fn in (
            self.test_heavy_tools_are_coroutines,
            self.test_heavy_tool_source_uses_to_thread,
        ):
            source = inspect.getsource(guard_fn).lstrip()
            tree = _ast.parse(source)
            for node in _ast.walk(tree):
                # We want chained attributes like ``crg_main.mcp.get_tools``.
                # That's an Attribute whose value is also an Attribute whose
                # attr == "mcp".
                if (
                    isinstance(node, _ast.Attribute)
                    and node.attr in forbidden_mcp_attrs
                    and isinstance(node.value, _ast.Attribute)
                    and node.value.attr == "mcp"
                ):
                    raise AssertionError(
                        f"{guard_fn.__name__} references mcp.{node.attr} — "
                        f"this attribute drifts across FastMCP releases "
                        f"and will silently break the guard.  Use "
                        f"getattr(crg_main, tool_name) instead."
                    )


class TestApplyToolFilter:
    """Tests for _apply_tool_filter (``serve --tools`` / ``CRG_TOOLS``).

    The filter removes MCP tools not present in the allow-list.
    Without an exact allow-list, every registered public tool stays exposed.
    """

    @pytest.fixture(autouse=True)
    def _restore_tools(self):
        """Snapshot registered tools before test, restore after.

        _apply_tool_filter calls ``mcp.remove_tool()`` which is
        permanent.  We restore by re-adding from the saved snapshot.
        """
        original = crg_main._snapshot_components()
        yield
        crg_main._restore_components(original)

    @pytest.fixture(autouse=True)
    def _clean_env(self, monkeypatch):
        """Ensure tool filter env vars are not set from the outer environment."""
        monkeypatch.delenv("CRG_TOOLS", raising=False)

    def test_no_filter_keeps_all_registered_tools(self):
        """When no exact allow-list is set, all public tools remain exposed."""
        before = _tool_names()
        crg_main._apply_tool_filter(None)
        after = _tool_names()
        assert after == before

    def test_architecture_dispatcher_replaces_split_public_tools(self):
        registered = _tool_names()
        assert "architecture_analysis_tool" in registered
        assert registered.isdisjoint(SPLIT_ARCHITECTURE_TOOL_NAMES)

    def test_review_and_flow_dispatchers_replace_split_public_tools(self):
        registered = _tool_names()
        assert "review_tool" in registered
        assert "flow_tool" in registered
        assert registered.isdisjoint(SPLIT_REVIEW_FLOW_TOOL_NAMES)

    def test_filter_via_argument(self):
        """The ``tools`` argument keeps only the listed tools."""
        keep = "query_graph_tool,semantic_search_nodes_tool"
        crg_main._apply_tool_filter(keep)
        remaining = _tool_names()
        assert remaining == {"query_graph_tool", "semantic_search_nodes_tool"}

    def test_filter_via_env_var(self, monkeypatch):
        """The ``CRG_TOOLS`` env var works as fallback."""
        monkeypatch.setenv("CRG_TOOLS", "query_graph_tool")
        crg_main._apply_tool_filter(None)
        remaining = _tool_names()
        assert remaining == {"query_graph_tool"}

    def test_argument_takes_precedence_over_env(self, monkeypatch):
        """CLI --tools wins over CRG_TOOLS env var."""
        monkeypatch.setenv("CRG_TOOLS", "list_repos_tool")
        crg_main._apply_tool_filter("query_graph_tool")
        remaining = _tool_names()
        assert remaining == {"query_graph_tool"}

    def test_empty_string_is_noop(self):
        """An empty string should not remove all tools."""
        before = _tool_names()
        crg_main._apply_tool_filter("")
        after = _tool_names()
        assert before == after

    def test_whitespace_handling(self):
        """Spaces around tool names are stripped."""
        crg_main._apply_tool_filter(" query_graph_tool , semantic_search_nodes_tool ")
        remaining = _tool_names()
        assert remaining == {"query_graph_tool", "semantic_search_nodes_tool"}
