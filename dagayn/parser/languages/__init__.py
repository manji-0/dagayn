"""Per-language walker-node handlers for the CodeParser tree walk."""

from . import bash, dart, elixir, julia, lua, r, solidity, terraform
from .markdown import parse as _parse_markdown
from .notebook import parse as _parse_notebook
from .notebook import parse_databricks_py as _parse_notebook_databricks
from .rescript import parse as _parse_rescript
from .svelte import parse as _parse_svelte
from .vue import parse as _parse_vue

SPECIAL_HANDLERS = {
    "r": r.handle_node,
    "lua": lua.handle_node,
    "luau": lua.handle_node,
    "julia": julia.handle_node,
    "bash": bash.handle_node,
    "elixir": elixir.handle_node,
    "dart": dart.handle_node,
    "solidity": solidity.handle_node,
    "terraform": terraform.handle_node,
}

__all__ = [
    "bash",
    "dart",
    "elixir",
    "julia",
    "lua",
    "r",
    "solidity",
    "terraform",
    "SPECIAL_HANDLERS",
    "_parse_vue",
    "_parse_svelte",
    "_parse_markdown",
    "_parse_notebook",
    "_parse_notebook_databricks",
    "_parse_rescript",
]
