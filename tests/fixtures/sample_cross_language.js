// Fixture for cross-language bridge detection tests (JavaScript).
// Uses canonical module-namespace forms; aliased imports require dataflow
// resolution and are intentionally not detected.

child_process.exec("git status");

child_process.execSync("./tools/lint --strict");

child_process.spawn("./target/release/dagayn-core", ["build", "."]);

child_process.spawnSync("./scripts/build.sh");

child_process.fork("./worker.js");

child_process.execFile("./bin/helper", ["--flag"]);

// Dynamic argument — should produce LOW confidence
function runDynamic(cmd) {
    child_process.exec(cmd);
}
