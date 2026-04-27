// Fixture for file-I/O bridge detection tests (JavaScript).
const fs = require("fs");

function readConfig() {
  return fs.readFileSync("config.yaml", "utf8");
}

function writeOutput() {
  fs.writeFileSync("output.json", "{}");
}

async function readAsync() {
  return await fs.promises.readFile("data/model.bin");
}

async function writeAsync() {
  await fs.promises.writeFile("reports/summary.txt", "done");
}

function readDynamic(path) {
  // Dynamic path — LOW confidence edge
  return fs.readFileSync(path, "utf8");
}
