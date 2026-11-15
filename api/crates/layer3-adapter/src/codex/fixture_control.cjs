const { writeFileSync, renameSync } = require("node:fs");

function writeFixtureControl(path, value, io = { writeFileSync, renameSync }) {
  // The proxy polls from another process. Rewriting the published inode can
  // expose empty/partial JSON and terminate it while an advisory reply is held.
  // Fixture controls have one synchronous writer and private scratch storage;
  // a same-directory rename makes each update visible as a complete snapshot.
  const pending = path + ".next";
  io.writeFileSync(pending, JSON.stringify(value));
  io.renameSync(pending, path);
}

module.exports = { writeFixtureControl };
