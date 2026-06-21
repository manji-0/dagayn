import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import Database from "better-sqlite3";

/**
 * SQL to create a minimal dagayn graph database for tests.
 * Mirrors the Python backend schema used by the extension.
 */
export const SCHEMA_SQL = `
CREATE TABLE IF NOT EXISTS nodes (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    name TEXT NOT NULL,
    qualified_name TEXT NOT NULL UNIQUE,
    file_path TEXT NOT NULL,
    line_start INTEGER,
    line_end INTEGER,
    language TEXT,
    parent_name TEXT,
    params TEXT,
    return_type TEXT,
    modifiers TEXT,
    is_test INTEGER DEFAULT 0,
    file_hash TEXT,
    extra TEXT DEFAULT '{}',
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    kind TEXT NOT NULL,
    source_qualified TEXT NOT NULL,
    target_qualified TEXT NOT NULL,
    file_path TEXT NOT NULL,
    line INTEGER DEFAULT 0,
    extra TEXT DEFAULT '{}',
    updated_at REAL NOT NULL
);

CREATE TABLE IF NOT EXISTS metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_nodes_file ON nodes(file_path);
CREATE INDEX IF NOT EXISTS idx_nodes_kind ON nodes(kind);
CREATE INDEX IF NOT EXISTS idx_nodes_qualified ON nodes(qualified_name);
CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_qualified);
CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_qualified);
CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind);
CREATE INDEX IF NOT EXISTS idx_edges_file ON edges(file_path);
`;

export interface TestNode {
  kind: string;
  name: string;
  qualified_name: string;
  file_path: string;
  line_start: number;
  line_end: number;
  language: string;
  parent_name: string | null;
  params: string | null;
  return_type: string | null;
  modifiers: string | null;
  is_test: number;
  file_hash: string;
  extra: string;
  updated_at: number;
}

export interface TestEdge {
  kind: string;
  source_qualified: string;
  target_qualified: string;
  file_path: string;
  line: number;
  extra: string;
  updated_at: number;
}

/**
 * Create a SQLite database with the test schema inside `.dagayn/graph.db`
 * under the given directory. The caller is responsible for deleting the
 * directory.
 */
export function buildTestGraphDb(
  parentDir: string,
  nodes: TestNode[] = [],
  edges: TestEdge[] = [],
): string {
  const dbDir = path.join(parentDir, ".dagayn");
  fs.mkdirSync(dbDir, { recursive: true });
  const dbPath = path.join(dbDir, "graph.db");

  const db = new Database(dbPath);
  db.exec(SCHEMA_SQL);

  const insertNode = db.prepare(`
    INSERT INTO nodes
      (kind, name, qualified_name, file_path, line_start, line_end,
       language, parent_name, params, return_type, modifiers, is_test,
       file_hash, extra, updated_at)
    VALUES
      (@kind, @name, @qualified_name, @file_path, @line_start, @line_end,
       @language, @parent_name, @params, @return_type, @modifiers, @is_test,
       @file_hash, @extra, @updated_at)
  `);

  const insertEdge = db.prepare(`
    INSERT INTO edges
      (kind, source_qualified, target_qualified, file_path, line, extra, updated_at)
    VALUES
      (@kind, @source_qualified, @target_qualified, @file_path, @line, @extra, @updated_at)
  `);

  const insertMany = db.transaction(() => {
    for (const n of nodes) {
      insertNode.run(n);
    }
    for (const e of edges) {
      insertEdge.run(e);
    }
  });
  insertMany();
  db.close();

  return dbPath;
}

/**
 * Create a temporary SQLite database with the test schema.
 * The caller is responsible for deleting the returned file/directory.
 */
export function buildTestDb(nodes: TestNode[] = [], edges: TestEdge[] = []): string {
  const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "dagayn-test-"));
  const dbPath = path.join(tmpDir, "graph.db");

  const db = new Database(dbPath);
  db.exec(SCHEMA_SQL);

  const insertNode = db.prepare(`
    INSERT INTO nodes
      (kind, name, qualified_name, file_path, line_start, line_end,
       language, parent_name, params, return_type, modifiers, is_test,
       file_hash, extra, updated_at)
    VALUES
      (@kind, @name, @qualified_name, @file_path, @line_start, @line_end,
       @language, @parent_name, @params, @return_type, @modifiers, @is_test,
       @file_hash, @extra, @updated_at)
  `);

  const insertEdge = db.prepare(`
    INSERT INTO edges
      (kind, source_qualified, target_qualified, file_path, line, extra, updated_at)
    VALUES
      (@kind, @source_qualified, @target_qualified, @file_path, @line, @extra, @updated_at)
  `);

  const insertMany = db.transaction(() => {
    for (const n of nodes) {
      insertNode.run(n);
    }
    for (const e of edges) {
      insertEdge.run(e);
    }
  });
  insertMany();
  db.close();

  return dbPath;
}
