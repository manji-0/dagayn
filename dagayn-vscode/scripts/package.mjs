#!/usr/bin/env node
// Build and package the extension into a VSIX.
//
// vsce discovers bundled dependencies by running `npm list --production`, which
// fails under pnpm's node_modules layout (ELSPROBLEMS). To keep the extension
// packageable from the repo's own pnpm toolchain we stage a clean production
// install with npm in a temp directory and run vsce from there.
//
// better-sqlite3 is a native module; `npm_config_target` pins the prebuild ABI
// to VS Code's runtime Node (22.x) so the packaged module loads in the editor
// even when the packager machine runs a newer Node.

import { execFileSync, execSync } from "node:child_process";
import { cpSync, existsSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const pkg = JSON.parse(readFileSync(path.join(root, "package.json"), "utf8"));
const outName = `${pkg.name}-${pkg.version}.vsix`;

// 1. Compile the extension and webview bundles.
execSync("node esbuild.mjs --production", { cwd: root, stdio: "inherit" });

// 2. Stage a minimal manifest (no devDependencies, no scripts) plus runtime
//    assets, then install production deps with npm so vsce's `npm list` works.
const staging = mkdtempSync(path.join(tmpdir(), "dagayn-vscode-package-"));
try {
  const manifest = { ...pkg, devDependencies: undefined, scripts: undefined };
  writeFileSync(
    path.join(staging, "package.json"),
    `${JSON.stringify(manifest, null, 2)}\n`,
  );
  for (const entry of [
    ".vscodeignore",
    "README.md",
    "CHANGELOG.md",
    "LICENSE",
    "dist",
    "media",
  ]) {
    cpSync(path.join(root, entry), path.join(staging, entry), { recursive: true });
  }
  if (existsSync(path.join(root, "NOTICE"))) {
    cpSync(path.join(root, "NOTICE"), path.join(staging, "NOTICE"));
  }

  execSync(
    "npm install --omit=dev --package-lock=false --no-audit --no-fund --ignore-scripts=false",
    {
      cwd: staging,
      stdio: "inherit",
      env: { ...process.env, npm_config_target: "22.0.0" },
    },
  );

  // 3. Package from the staging dir with the repo's vsce binary.
  const vsceBin = path.join(root, "node_modules", ".bin", "vsce");
  execFileSync(vsceBin, ["package", "--out", path.join(root, outName)], {
    cwd: staging,
    stdio: "inherit",
  });
} finally {
  rmSync(staging, { recursive: true, force: true });
}

console.log(`Packaged ${outName}`);
