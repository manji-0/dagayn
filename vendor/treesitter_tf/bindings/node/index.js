try {
  module.exports = require("../../build/Release/tree_sitter_terraform_binding");
} catch (_) {
  module.exports = require("../../build/Debug/tree_sitter_terraform_binding");
}
try {
  module.exports.nodeTypeInfo = require("../../src/node-types.json");
} catch (_) {}
