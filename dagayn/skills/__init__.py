"""Claude Code skills and hooks auto-install.

Generates Claude Code agent skill files, hooks configuration, and
CLAUDE.md integration for seamless dagayn usage.
Also supports multi-platform MCP server installation and
Cursor hooks / OpenCode plugin generation.
"""

from .cursor import (
    generate_cursor_hooks_config,
    install_cursor_hooks,
    install_cursor_worktree_setup,
)
from .hooks import (
    generate_hermes_hooks_config,
    generate_hooks_config,
    generate_pi_hooks_config,
    install_codex_hooks,
    install_git_hook,
    install_hermes_hooks,
    install_hooks,
    install_pi_hooks,
)
from .instructions import (
    ensure_worktree_include,
    inject_claude_md,
    inject_platform_instructions,
    worktree_include_patterns,
)
from .opencode import (
    install_opencode_plugin,
)
from .platforms import (
    PLATFORMS,
    install_platform_configs,
    normalize_platform_target,
)
from .skill_files import (
    generate_skills,
    install_codex_skills,
    install_global_skills,
    install_hermes_skills,
    install_opencode_skills,
    install_pi_skills,
    install_qoder_skills,
)

__all__ = [
    "PLATFORMS",
    "ensure_worktree_include",
    "generate_cursor_hooks_config",
    "generate_hermes_hooks_config",
    "generate_hooks_config",
    "generate_pi_hooks_config",
    "generate_skills",
    "inject_claude_md",
    "inject_platform_instructions",
    "install_codex_hooks",
    "install_codex_skills",
    "install_cursor_hooks",
    "install_cursor_worktree_setup",
    "install_git_hook",
    "install_global_skills",
    "install_hermes_hooks",
    "install_hermes_skills",
    "install_hooks",
    "install_opencode_plugin",
    "install_opencode_skills",
    "install_pi_hooks",
    "install_pi_skills",
    "install_platform_configs",
    "install_qoder_skills",
    "normalize_platform_target",
    "worktree_include_patterns",
]
