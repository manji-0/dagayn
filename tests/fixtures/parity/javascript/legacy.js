const path = require("path");
const { helper } = require("./helpers");
const helpers = require("./helpers");
class Legacy extends Base {
  handle = () => { helper(); };
  static make() { return new Legacy(); }
  run() { helpers.other(); this.handle(); }
}
function Base() {}
module.exports = { Legacy };
module.exports.fn = function namedExport() {};
exports.short = () => 1;
