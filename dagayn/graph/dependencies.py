from __future__ import annotations

import logging

from ..bare_name_resolution import (
    resolve_bare_call_targets as _resolve_bare_call_targets,
)
from ..bare_name_resolution import (
    resolve_bare_inheritance_targets as _resolve_bare_inheritance_targets,
)
from ._mixin_protocol import GraphStoreMixinProtocol
from .types import TransitiveTestRecord

logger = logging.getLogger(__name__)


class GraphStoreDependencyMixin(GraphStoreMixinProtocol):
    def get_direct_dependents(self, file_paths: list[str]) -> list[str]:
        """Return files that directly depend on any of *file_paths*."""
        if not file_paths:
            return []

        dependents: set[str] = set()
        fp_keys: list[str] = []
        seen_keys: set[str] = set()
        for file_path in file_paths:
            normalized = self._normalize_qualified_key(file_path)
            for key in (file_path, normalized):
                if key not in seen_keys:
                    seen_keys.add(key)
                    fp_keys.append(key)

        batch_size = 450
        for i in range(0, len(fp_keys), batch_size):
            chunk = fp_keys[i : i + batch_size]
            placeholders = ",".join("?" for _ in chunk)
            rows = self._conn.execute(
                "SELECT file_path FROM edges "
                f"WHERE target_qualified IN ({placeholders}) AND kind = 'IMPORTS_FROM'",
                chunk,
            ).fetchall()
            dependents.update(row["file_path"] for row in rows)

        node_qns: list[str] = []
        for i in range(0, len(fp_keys), batch_size):
            chunk = fp_keys[i : i + batch_size]
            placeholders = ",".join("?" for _ in chunk)
            rows = self._conn.execute(
                f"SELECT qualified_name FROM nodes WHERE file_path IN ({placeholders})",
                chunk,
            ).fetchall()
            node_qns.extend(row["qualified_name"] for row in rows)

        for i in range(0, len(node_qns), batch_size):
            chunk = node_qns[i : i + batch_size]
            placeholders = ",".join("?" for _ in chunk)
            rows = self._conn.execute(
                "SELECT DISTINCT file_path FROM edges "
                f"WHERE target_qualified IN ({placeholders}) "
                "AND kind IN ('CALLS', 'IMPORTS_FROM', 'INHERITS', 'IMPLEMENTS')",
                chunk,
            ).fetchall()
            dependents.update(row["file_path"] for row in rows)

        dependents.difference_update(file_paths)
        return sorted(dependents)

    def get_transitive_tests(
        self,
        qualified_name: str,
        max_depth: int = 1,
    ) -> list[TransitiveTestRecord]:
        """Find tests covering a node, including indirect (transitive) coverage.

        1. Direct: TESTED_BY edges sourced from this node (bare-name fallback only
           when the qualified lookup finds no direct coverage).
        2. Indirect: follow outgoing CALLS edges up to *max_depth* hops,
           then collect TESTED_BY edges on each callee.

        Returns a list of dicts with node fields plus ``indirect: bool``.
        """
        conn = self._conn
        seen: set[str] = set()
        results: list[TransitiveTestRecord] = []

        # If the input is a class, expand to its methods first.
        input_qns = [qualified_name]
        row = conn.execute(
            "SELECT kind FROM nodes WHERE qualified_name = ?",
            (qualified_name,),
        ).fetchone()
        if row and row["kind"] == "Class":
            for mrow in conn.execute(
                "SELECT target_qualified FROM edges "
                "WHERE source_qualified = ? AND kind = 'CONTAINS'",
                (qualified_name,),
            ).fetchall():
                input_qns.append(mrow["target_qualified"])

        def _node_dict(qn: str, indirect: bool) -> TransitiveTestRecord | None:
            row = conn.execute("SELECT * FROM nodes WHERE qualified_name = ?", (qn,)).fetchone()
            if not row:
                return None
            return {
                "name": row["name"],
                "qualified_name": row["qualified_name"],
                "file_path": row["file_path"],
                "kind": row["kind"],
                "indirect": indirect,
            }

        # Direct TESTED_BY (production node -> test node)
        found_qualified_direct = False
        for qn in input_qns:
            for row in conn.execute(
                "SELECT target_qualified FROM edges "
                "WHERE source_qualified = ? AND kind = 'TESTED_BY'",
                (qn,),
            ).fetchall():
                found_qualified_direct = True
                test_qn = row["target_qualified"]
                if test_qn not in seen:
                    seen.add(test_qn)
                    d = _node_dict(test_qn, indirect=False)
                    if d:
                        results.append(d)

        # Bare-name fallback only when qualified lookup found no direct coverage.
        if not found_qualified_direct:
            bare = qualified_name.rsplit("::", 1)[-1] if "::" in qualified_name else qualified_name
            for row in conn.execute(
                "SELECT target_qualified FROM edges "
                "WHERE source_qualified = ? AND kind = 'TESTED_BY'",
                (bare,),
            ).fetchall():
                test_qn = row["target_qualified"]
                if test_qn not in seen:
                    seen.add(test_qn)
                    d = _node_dict(test_qn, indirect=False)
                    if d:
                        results.append(d)

        # Transitive: follow CALLS edges, then collect TESTED_BY on callees
        frontier = set(input_qns)
        for _ in range(max_depth):
            next_frontier: set[str] = set()
            for qn in frontier:
                for row in conn.execute(
                    "SELECT target_qualified FROM edges "
                    "WHERE source_qualified = ? AND kind = 'CALLS'",
                    (qn,),
                ).fetchall():
                    next_frontier.add(row["target_qualified"])
            for callee in next_frontier:
                for row in conn.execute(
                    "SELECT target_qualified FROM edges "
                    "WHERE source_qualified = ? AND kind = 'TESTED_BY'",
                    (callee,),
                ).fetchall():
                    test_qn = row["target_qualified"]
                    if test_qn not in seen:
                        seen.add(test_qn)
                        d = _node_dict(test_qn, indirect=True)
                        if d:
                            results.append(d)
            frontier = next_frontier

        return results

    def resolve_bare_call_targets(self) -> int:
        """Batch-resolve bare-name CALLS targets using import-aware disambiguation."""
        return _resolve_bare_call_targets(self)

    def resolve_bare_inheritance_targets(self) -> int:
        """Batch-resolve bare-name INHERITS/IMPLEMENTS targets using import context."""
        return _resolve_bare_inheritance_targets(self)
