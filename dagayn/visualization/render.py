"""HTML generation for the code graph visualization."""

from __future__ import annotations

import json
import logging
from importlib.resources import files
from pathlib import Path

from ..graph import GraphStore
from .aggregate import _aggregate_community, _aggregate_file
from .data import export_graph_data

logger = logging.getLogger(__name__)

_TEMPLATES = files("dagayn.visualization.templates")


def _load_template(name: str) -> str:
    return (_TEMPLATES / name).read_text(encoding="utf-8")


def generate_html(
    store: GraphStore,
    output_path: str | Path,
    mode: str = "auto",
    max_full_nodes: int = 3000,
) -> Path:
    """Generate a self-contained interactive HTML visualization.

    Args:
        store: The GraphStore to read graph data from.
        output_path: Path for the output HTML file.
        mode: Rendering mode — ``"auto"``, ``"full"``, ``"community"``,
              or ``"file"``.  ``"auto"`` switches to ``"community"`` when
              the node count exceeds *max_full_nodes*.
        max_full_nodes: Threshold for auto-switching to community mode.

    Writes the HTML file to *output_path* and returns the resolved Path.
    """
    output_path = Path(output_path)
    stats = store.get_stats()
    if stats.total_nodes > 50000:
        logger.warning(
            "Graph has %d nodes — visualization may be slow. Consider filtering by file pattern.",
            stats.total_nodes,
        )
    data = export_graph_data(store)

    # Determine effective mode
    effective_mode = mode
    if effective_mode == "auto":
        effective_mode = "community" if stats.total_nodes > max_full_nodes else "full"

    if effective_mode == "community":
        agg = _aggregate_community(data)
        data_json = json.dumps(agg, default=str).replace("</", "<\\/")
        html = _load_template("aggregated.html").replace("__GRAPH_DATA__", data_json)
    elif effective_mode == "file":
        agg = _aggregate_file(data)
        data_json = json.dumps(agg, default=str).replace("</", "<\\/")
        html = _load_template("aggregated.html").replace("__GRAPH_DATA__", data_json)
    else:
        # full mode — original behavior
        data_json = json.dumps(data, default=str).replace("</", "<\\/")
        html = _load_template("full.html").replace("__GRAPH_DATA__", data_json)

    output_path.write_text(html, encoding="utf-8")
    return output_path
