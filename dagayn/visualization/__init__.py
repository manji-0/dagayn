"""Interactive D3.js graph visualization for code knowledge graphs.

Exports graph data to JSON and generates a self-contained HTML file with
a force-directed D3.js visualization. Dark theme, zoomable, draggable,
with collapsible file clusters, tooltips, legend, and stats bar.

Supports multiple rendering modes for large graphs:
- ``full``  — render every node (default, current behavior)
- ``community`` — aggregate by community; double-click to drill down
- ``file``  — aggregate by file; each file is a node
- ``auto``  — choose community mode when node count exceeds threshold
"""

from .aggregate import _aggregate_community, _aggregate_file
from .data import export_graph_data
from .render import generate_html

__all__ = [
    "_aggregate_community",
    "_aggregate_file",
    "export_graph_data",
    "generate_html",
]
