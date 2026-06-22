import * as fs from "node:fs/promises";
import * as path from "node:path";

export type SavedQuery = {
  label: string;
  pattern: string;
  target: string;
};

export type SavedQueriesFile = {
  schemaVersion: 1;
  queries: SavedQuery[];
};

export const QUERIES_PATH = ".dagayn/queries.json";

export function resolveQueriesPath(workspaceFsPath: string): string {
  return path.join(workspaceFsPath, QUERIES_PATH);
}

function isSavedQuery(value: unknown): value is SavedQuery {
  return (
    typeof value === "object" &&
    value !== null &&
    "label" in value &&
    typeof (value as SavedQuery).label === "string" &&
    "pattern" in value &&
    typeof (value as SavedQuery).pattern === "string" &&
    "target" in value &&
    typeof (value as SavedQuery).target === "string"
  );
}

function isSavedQueriesFile(value: unknown): value is SavedQueriesFile {
  return (
    typeof value === "object" &&
    value !== null &&
    "schemaVersion" in value &&
    (value as SavedQueriesFile).schemaVersion === 1 &&
    "queries" in value &&
    Array.isArray((value as SavedQueriesFile).queries) &&
    (value as SavedQueriesFile).queries.every(isSavedQuery)
  );
}

async function readQueriesFile(workspaceFsPath: string): Promise<SavedQueriesFile | undefined> {
  const filePath = resolveQueriesPath(workspaceFsPath);
  try {
    const raw = await fs.readFile(filePath, "utf-8");
    const parsed: unknown = JSON.parse(raw);
    if (!isSavedQueriesFile(parsed)) {
      throw new Error(`Saved queries file is corrupted: ${filePath}. Fix or delete it.`);
    }
    return parsed;
  } catch (err: unknown) {
    if (isMissingError(err)) {
      return undefined;
    }
    if (err instanceof SyntaxError) {
      throw new Error(`Saved queries file is corrupted: ${filePath}. Fix or delete it.`);
    }
    throw err;
  }
}

function isMissingError(err: unknown): boolean {
  return (
    typeof err === "object" &&
    err !== null &&
    "code" in err &&
    (err as { code: string }).code === "ENOENT"
  );
}

export async function loadSavedQueries(
  workspaceFsPath: string,
  validPatterns: string[],
): Promise<SavedQuery[]> {
  const file = await readQueriesFile(workspaceFsPath);
  if (!file) {
    return [];
  }
  const valid = new Set(validPatterns);
  return file.queries.filter((q) => valid.has(q.pattern));
}

export async function saveSavedQuery(
  workspaceFsPath: string,
  query: SavedQuery,
): Promise<SavedQuery[]> {
  const filePath = resolveQueriesPath(workspaceFsPath);
  await fs.mkdir(path.dirname(filePath), { recursive: true });

  const existing = (await readQueriesFile(workspaceFsPath))?.queries ?? [];
  const idx = existing.findIndex((q) => q.label === query.label);
  const next = [...existing];
  if (idx >= 0) {
    next[idx] = query;
  } else {
    next.push(query);
  }

  const payload: SavedQueriesFile = { schemaVersion: 1, queries: next };
  const tempPath = `${filePath}.tmp`;
  await fs.writeFile(tempPath, JSON.stringify(payload, null, 2), "utf-8");
  await fs.rename(tempPath, filePath);

  return next;
}

export async function deleteSavedQuery(
  workspaceFsPath: string,
  label: string,
): Promise<SavedQuery[]> {
  const filePath = resolveQueriesPath(workspaceFsPath);
  const existing = (await readQueriesFile(workspaceFsPath))?.queries ?? [];
  const next = existing.filter((q) => q.label !== label);
  if (next.length === existing.length) {
    return next;
  }

  const payload: SavedQueriesFile = { schemaVersion: 1, queries: next };
  const tempPath = `${filePath}.tmp`;
  await fs.writeFile(tempPath, JSON.stringify(payload, null, 2), "utf-8");
  await fs.rename(tempPath, filePath);

  return next;
}
