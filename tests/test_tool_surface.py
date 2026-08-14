"""Tests for the active MCP tool-surface allow-list (#107)."""

from dagayn.tool_surface import (
    filter_suggestions,
    filter_tool_names,
    set_active_tool_surface,
    suggestion_is_callable,
    tool_is_exposed,
)


class TestToolSurfaceFilter:
    def setup_method(self):
        set_active_tool_surface(None)

    def teardown_method(self):
        set_active_tool_surface(None)

    def test_unrestricted_exposes_every_name(self):
        assert tool_is_exposed("apply_refactor_tool")
        assert suggestion_is_callable("apply_refactor_tool(refactor_id='x')")

    def test_allow_list_hides_other_mcp_tools(self):
        set_active_tool_surface({"refactor_tool", "query_graph_tool"})
        assert tool_is_exposed("refactor_tool")
        assert not tool_is_exposed("apply_refactor_tool")
        assert not suggestion_is_callable(
            "apply_refactor_tool(refactor_id='x') -- apply the rename"
        )

    def test_cli_suggestions_remain_callable(self):
        set_active_tool_surface({"refactor_tool"})
        assert suggestion_is_callable("dagayn tool apply_refactor_tool --arg refactor_id='x'")
        assert suggestion_is_callable("Run: dagayn register /path")

    def test_filter_tool_names_and_suggestions(self):
        set_active_tool_surface({"review_tool", "flow_tool"})
        assert filter_tool_names(["review_tool", "find_large_functions_tool"]) == ["review_tool"]
        assert filter_suggestions(
            [
                "review_tool -- inspect changes",
                "find_large_functions_tool -- size audit",
                "dagayn tool apply_refactor_tool --arg refactor_id='x'",
            ]
        ) == [
            "review_tool -- inspect changes",
            "dagayn tool apply_refactor_tool --arg refactor_id='x'",
        ]
