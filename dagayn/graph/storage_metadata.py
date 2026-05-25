from __future__ import annotations

from typing import Optional

from ._mixin_protocol import GraphStoreMixinProtocol


class GraphStoreStorageMetadataMixin(GraphStoreMixinProtocol):
    def set_metadata(self, key: str, value: str) -> None:
        self._conn.execute(
            "INSERT OR REPLACE INTO metadata (key, value) VALUES (?, ?)", (key, value)
        )
        self._conn.commit()

    def get_metadata(self, key: str) -> Optional[str]:
        row = self._conn.execute("SELECT value FROM metadata WHERE key=?", (key,)).fetchone()
        return row["value"] if row else None

    def commit(self) -> None:
        self._conn.commit()

    def rollback(self) -> None:
        """Rollback the current transaction."""
        self._conn.rollback()

    def get_file_meta_map(self) -> dict[str, tuple[str, int]]:
        """Return ``{file_path: (file_hash, mtime_ns)}`` for all files with stored nodes.

        Used by ``incremental_update`` to skip reading file bytes when the
        stored mtime_ns matches the current filesystem mtime_ns.
        """
        rows = self._conn.execute(
            "SELECT DISTINCT file_path, file_hash, mtime_ns FROM nodes "
            "WHERE file_hash IS NOT NULL AND file_hash != ''"
        ).fetchall()
        return {r["file_path"]: (r["file_hash"] or "", r["mtime_ns"] or 0) for r in rows}

    def get_file_meta_for_files(self, file_paths: list[str]) -> dict[str, tuple[str, int]]:
        """Return stored ``(file_hash, mtime_ns)`` metadata for selected files."""
        out: dict[str, tuple[str, int]] = {}
        for i in range(0, len(file_paths), 450):
            chunk = file_paths[i : i + 450]
            if not chunk:
                continue
            placeholders = ",".join("?" for _ in chunk)
            rows = self._conn.execute(
                "SELECT DISTINCT file_path, file_hash, mtime_ns FROM nodes "
                "WHERE file_hash IS NOT NULL AND file_hash != '' "
                f"AND file_path IN ({placeholders})",
                chunk,
            ).fetchall()
            out.update({r["file_path"]: (r["file_hash"] or "", r["mtime_ns"] or 0) for r in rows})
        return out

    def update_file_mtime(self, file_path: str, mtime_ns: int) -> None:
        """Update mtime_ns for all nodes belonging to *file_path*."""
        self._conn.execute("UPDATE nodes SET mtime_ns=? WHERE file_path=?", (mtime_ns, file_path))

    def update_file_mtimes(self, updates: list[tuple[int, str]]) -> None:
        """Update mtime_ns for multiple files."""
        self._conn.executemany("UPDATE nodes SET mtime_ns=? WHERE file_path=?", updates)
