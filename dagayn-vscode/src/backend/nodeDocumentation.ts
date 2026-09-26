const DOCUMENTATION_KEYS = ["docstring", "doc", "comment", "comments"] as const;

/**
 * Extract a human-readable documentation string from a node's `extra` object.
 *
 * Priority order: docstring > doc > comment > comments.
 * Arrays under `comments` are joined with newlines. Empty values are ignored.
 */
export function getNodeDocumentation(
  node: { extra?: Record<string, unknown> } | undefined,
): string {
  if (!node?.extra || typeof node.extra !== "object") {
    return "";
  }

  for (const key of DOCUMENTATION_KEYS) {
    const raw = node.extra[key];
    if (raw === undefined || raw === null) {
      continue;
    }

    if (key === "comments" && Array.isArray(raw)) {
      const joined = raw
        .filter((item): item is string => typeof item === "string")
        .join("\n")
        .trim();
      if (joined.length > 0) {
        return joined;
      }
      continue;
    }

    if (typeof raw === "string") {
      const trimmed = raw.trim();
      if (trimmed.length > 0) {
        return trimmed;
      }
    }
  }

  return "";
}
