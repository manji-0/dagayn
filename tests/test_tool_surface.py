"""Tests for the active MCP tool-surface allow-list (#107)."""

from dagayn.tool_surface import set_active_tool_surface, tool_is_exposed


class TestToolSurfaceFilter:
    def setup_method(self):
        set_active_tool_surface(None)

    def teardown_method(self):
        set_active_tool_surface(None)

    def test_unrestricted_exposes_every_name(self):
        assert tool_is_exposed("apply_refactor_tool")

    def test_allow_list_hides_other_mcp_tools(self):
        set_active_tool_surface({"refactor_tool", "query_graph_tool"})
        assert tool_is_exposed("refactor_tool")
        assert not tool_is_exposed("apply_refactor_tool")
