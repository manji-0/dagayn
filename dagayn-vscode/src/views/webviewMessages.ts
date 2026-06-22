export type IncomingWebviewMessage =
  | { command: "ready" }
  | {
      command: "nodeClicked";
      qualifiedName?: string;
      filePath: string;
      lineStart: number | null | undefined;
      kind: string;
    }
  | { command: "exportSvg"; svg: string }
  | { command: "exportPng"; data: string };

export type OutgoingWebviewMessage =
  | { command: "setData"; nodes: unknown[]; edges: unknown[]; truncated: boolean; maxNodes: number }
  | { command: "setTheme"; theme: "dark" | "light" }
  | { command: "highlightNode"; qualifiedName: string };

function isString(value: unknown): value is string {
  return typeof value === "string";
}

function isNumberOrNullOrUndefined(value: unknown): value is number | null | undefined {
  return (
    value === null || value === undefined || (typeof value === "number" && Number.isFinite(value))
  );
}

/**
 * Validate and narrow an incoming webview message.
 *
 * Returns the typed message, or null if the payload is malformed.
 */
export function parseIncomingWebviewMessage(raw: unknown): IncomingWebviewMessage | null {
  if (!raw || typeof raw !== "object") {
    return null;
  }

  const message = raw as Record<string, unknown>;
  if (!isString(message.command)) {
    return null;
  }

  switch (message.command) {
    case "ready": {
      return { command: "ready" };
    }

    case "nodeClicked": {
      if (!isString(message.filePath) || !isString(message.kind)) {
        return null;
      }
      if (message.qualifiedName !== undefined && !isString(message.qualifiedName)) {
        return null;
      }
      if (!isNumberOrNullOrUndefined(message.lineStart)) {
        return null;
      }
      return {
        command: "nodeClicked",
        filePath: message.filePath,
        lineStart: message.lineStart,
        kind: message.kind,
        ...(message.qualifiedName !== undefined ? { qualifiedName: message.qualifiedName } : {}),
      };
    }

    case "exportSvg": {
      if (!isString(message.svg)) {
        return null;
      }
      return { command: "exportSvg", svg: message.svg };
    }

    case "exportPng": {
      if (!isString(message.data)) {
        return null;
      }
      return { command: "exportPng", data: message.data };
    }

    default: {
      return null;
    }
  }
}
