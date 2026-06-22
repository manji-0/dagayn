import * as assert from "node:assert";
import * as fs from "node:fs/promises";
import * as os from "node:os";
import * as path from "node:path";
import {
  deleteSavedQuery,
  loadSavedQueries,
  QUERIES_PATH,
  saveSavedQuery,
} from "../src/features/savedQueries";

const VALID_PATTERNS = ["callers_of", "callees_of", "tests_for"];

describe("savedQueries", () => {
  let tmpRoot: string;

  beforeEach(async () => {
    tmpRoot = await fs.mkdtemp(path.join(os.tmpdir(), "dagayn-queries-test-"));
  });

  afterEach(async () => {
    await fs.rm(tmpRoot, { recursive: true, force: true });
  });

  function queriesPath(): string {
    return path.join(tmpRoot, QUERIES_PATH);
  }

  it("returns an empty array when the file is missing", async () => {
    const result = await loadSavedQueries(tmpRoot, VALID_PATTERNS);
    assert.deepStrictEqual(result, []);
  });

  it("creates .dagayn/ and persists a query", async () => {
    const saved = await saveSavedQuery(tmpRoot, {
      label: "callers of login",
      pattern: "callers_of",
      target: "src/auth.py::login",
    });

    assert.strictEqual(saved.length, 1);
    assert.strictEqual(saved[0].label, "callers of login");

    const stat = await fs.stat(queriesPath());
    assert.ok(stat.isFile());

    const loaded = await loadSavedQueries(tmpRoot, VALID_PATTERNS);
    assert.strictEqual(loaded.length, 1);
    assert.deepStrictEqual(loaded[0], saved[0]);
  });

  it("overwrites an existing query with the same label", async () => {
    await saveSavedQuery(tmpRoot, {
      label: "callers of login",
      pattern: "callers_of",
      target: "src/auth.py::login",
    });

    const saved = await saveSavedQuery(tmpRoot, {
      label: "callers of login",
      pattern: "callees_of",
      target: "src/auth.py::logout",
    });

    assert.strictEqual(saved.length, 1);
    assert.strictEqual(saved[0].pattern, "callees_of");

    const loaded = await loadSavedQueries(tmpRoot, VALID_PATTERNS);
    assert.strictEqual(loaded.length, 1);
    assert.strictEqual(loaded[0].pattern, "callees_of");
  });

  it("throws on corrupted JSON", async () => {
    await fs.mkdir(path.dirname(queriesPath()), { recursive: true });
    await fs.writeFile(queriesPath(), "{ not json", "utf-8");

    await assert.rejects(async () => loadSavedQueries(tmpRoot, VALID_PATTERNS), /corrupted/);
  });

  it("throws on an unsupported schema version", async () => {
    await fs.mkdir(path.dirname(queriesPath()), { recursive: true });
    await fs.writeFile(queriesPath(), JSON.stringify({ schemaVersion: 2, queries: [] }), "utf-8");

    await assert.rejects(async () => loadSavedQueries(tmpRoot, VALID_PATTERNS), /corrupted/);
  });

  it("filters out invalid patterns and keeps valid ones", async () => {
    await saveSavedQuery(tmpRoot, {
      label: "callers",
      pattern: "callers_of",
      target: "src/auth.py::login",
    });
    await saveSavedQuery(tmpRoot, {
      label: "unknown",
      pattern: "future_pattern",
      target: "src/auth.py::login",
    });

    const loaded = await loadSavedQueries(tmpRoot, VALID_PATTERNS);
    assert.strictEqual(loaded.length, 1);
    assert.strictEqual(loaded[0].label, "callers");
  });

  it("deletes a query by label and is a no-op for unknown labels", async () => {
    await saveSavedQuery(tmpRoot, {
      label: "callers",
      pattern: "callers_of",
      target: "src/auth.py::login",
    });

    const afterDelete = await deleteSavedQuery(tmpRoot, "callers");
    assert.strictEqual(afterDelete.length, 0);

    const loaded = await loadSavedQueries(tmpRoot, VALID_PATTERNS);
    assert.strictEqual(loaded.length, 0);

    const noOp = await deleteSavedQuery(tmpRoot, "missing");
    assert.strictEqual(noOp.length, 0);
  });
});
