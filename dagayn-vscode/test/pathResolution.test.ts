import * as assert from "node:assert";
import * as fs from "node:fs";
import * as os from "node:os";
import * as path from "node:path";
import { describe, it, afterEach } from "mocha";
import { resolveNodeFilePath } from "../src/backend/pathResolution";

describe("resolveNodeFilePath", () => {
  let tmpDirs: string[] = [];

  afterEach(() => {
    for (const dir of tmpDirs) {
      try {
        fs.rmSync(dir, { recursive: true, force: true });
      } catch {
        // best effort
      }
    }
    tmpDirs = [];
  });

  function makeDir(): string {
    const dir = fs.mkdtempSync(path.join(os.tmpdir(), "dagayn-path-test-"));
    tmpDirs.push(dir);
    return dir;
  }

  function makeFile(dir: string, relPath: string): string {
    const full = path.join(dir, relPath);
    fs.mkdirSync(path.dirname(full), { recursive: true });
    fs.writeFileSync(full, "test");
    return full;
  }

  it("returns an absolute existing path as-is", () => {
    const dir = makeDir();
    const file = makeFile(dir, "src/auth.py");
    const result = resolveNodeFilePath(file, []);
    assert.strictEqual(result.candidate, file);
    assert.deepStrictEqual(result.tried, [file]);
  });

  it("resolves a relative path against the first matching workspace folder", () => {
    const dir = makeDir();
    const file = makeFile(dir, "src/auth.py");
    const result = resolveNodeFilePath("src/auth.py", [dir]);
    assert.strictEqual(result.candidate, file);
    assert.deepStrictEqual(result.tried, [path.join(dir, "src/auth.py")]);
  });

  it("picks the longest matching folder first to support nested roots", () => {
    const parent = makeDir();
    const nested = path.join(parent, "nested");
    fs.mkdirSync(nested, { recursive: true });
    tmpDirs.push(nested); // keep cleanup happy
    const file = makeFile(nested, "src/auth.py");

    const result = resolveNodeFilePath("src/auth.py", [parent, nested]);
    assert.strictEqual(result.candidate, file);
    assert.strictEqual(result.tried[0], path.join(nested, "src/auth.py"));
  });

  it("falls through to a shorter folder when the longest one misses", () => {
    const parent = makeDir();
    const nested = path.join(parent, "nested");
    fs.mkdirSync(nested, { recursive: true });
    tmpDirs.push(nested);
    const file = makeFile(parent, "src/auth.py");

    const result = resolveNodeFilePath("src/auth.py", [nested, parent]);
    assert.strictEqual(result.candidate, file);
    assert.deepStrictEqual(result.tried, [
      path.join(nested, "src/auth.py"),
      path.join(parent, "src/auth.py"),
    ]);
  });

  it("returns undefined and lists all candidates when no path exists", () => {
    const dirA = makeDir();
    const dirB = makeDir();
    const result = resolveNodeFilePath("missing/file.py", [dirA, dirB]);
    assert.strictEqual(result.candidate, undefined);
    assert.deepStrictEqual(result.tried, [
      path.join(dirA, "missing/file.py"),
      path.join(dirB, "missing/file.py"),
    ]);
  });
});
