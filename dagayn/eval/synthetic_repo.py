"""Generate synthetic Python repositories for scale benchmarks.

The generator is deterministic and aims for an approximate node budget by
emitting one File node plus a class and a chain of functions per module.
Embeddings are never generated here — graph construction is measured alone.
"""

from __future__ import annotations

from pathlib import Path


def estimate_nodes_per_module(functions_per_module: int) -> int:
    """File + Class + functions + a module-level ``entry`` wrapper."""
    return 2 + functions_per_module + 1


def module_layout(target_nodes: int, functions_per_module: int = 20) -> tuple[int, int]:
    """Return ``(module_count, functions_per_module)`` for *target_nodes*."""
    per = estimate_nodes_per_module(functions_per_module)
    modules = max(2, (target_nodes + per - 1) // per)
    return modules, functions_per_module


def write_synthetic_python_repo(
    root: Path,
    *,
    target_nodes: int = 10_000,
    functions_per_module: int = 20,
) -> dict[str, int]:
    """Write a callable synthetic package under *root*.

    Returns the planned layout (not parsed graph stats).
    """
    root.mkdir(parents=True, exist_ok=True)
    pkg = root / "synth"
    pkg.mkdir(parents=True, exist_ok=True)
    (pkg / "__init__.py").write_text("", encoding="utf-8")

    modules, funcs = module_layout(target_nodes, functions_per_module)
    for index in range(modules):
        next_mod = (index + 1) % modules
        lines = [
            f"from synth.m{next_mod:04d} import entry as next_entry",
            "",
            f"class Worker{index:04d}:",
            "    def run(self, value: int) -> int:",
            "        return step_0(value)",
            "",
        ]
        for fn_i in range(funcs):
            nxt = f"step_{fn_i + 1}" if fn_i + 1 < funcs else "next_entry"
            lines.extend(
                [
                    f"def step_{fn_i}(value: int) -> int:",
                    f"    return {nxt}(value + {fn_i})",
                    "",
                ]
            )
        lines.extend(
            [
                "def entry(value: int = 0) -> int:",
                "    return Worker{0:04d}().run(value)".format(index),
                "",
            ]
        )
        (pkg / f"m{index:04d}.py").write_text("\n".join(lines), encoding="utf-8")

    (root / "README.md").write_text(
        f"# synth\n\nSynthetic dagayn scale fixture targeting ~{target_nodes} nodes.\n",
        encoding="utf-8",
    )
    return {
        "target_nodes": target_nodes,
        "modules": modules,
        "functions_per_module": funcs,
        "planned_nodes": modules * estimate_nodes_per_module(funcs),
    }
