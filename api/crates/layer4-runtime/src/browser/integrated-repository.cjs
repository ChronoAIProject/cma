const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdir, writeFile } = require("node:fs/promises");

exports.prepare = async function prepare(atomic) {
  if (process.env.CMA_INTEGRATED_REPOSITORY !== "1") return undefined;
  const root = "/tmp/integrated-source";
  await mkdir(root);
  const git = (args) => execFileSync("git", args, { cwd: root });
  git(["init", "-b", "proof/fixed", "."]);
  const content =
    "Integrated comparison source\nTool A: simple\nTool B: flexible\nTool C: portable\n";
  await writeFile(root + "/README.md", content);
  git(["add", "README.md"]);
  git([
    "-c",
    "user.name=Ticket19 proof",
    "-c",
    "user.email=proof@example.invalid",
    "commit",
    "-m",
    "Isolated comparison source",
  ]);
  const commit = git(["rev-parse", "HEAD"]).toString().trim();
  const archive = git([
    "archive",
    "--format=zip",
    "--prefix=comparison/",
    "HEAD",
  ]);
  await writeFile(
    process.env.CMA_DESKTOP_RELAY_DIRECTORY + "/repository.zip",
    archive,
  );
  git([
    "bundle",
    "create",
    process.env.CMA_DESKTOP_RELAY_DIRECTORY + "/repository.bundle",
    "--all",
  ]);
  const requests = [];
  await atomic("repository.json", {
    commit,
    content,
    source: "Actual owned Git commit; synthetic GitHub API boundary",
    requests,
  });
  return {
    commit,
    requests,
    handle(request, response) {
      const prefix = "/api/v1/proxy/s/api-github/repos/ticket19/comparison";
      if (!request.url.startsWith(prefix)) return false;
      const url = new URL(request.url, "http://127.0.0.1");
      requests.push(url.pathname + url.search);
      const json = (value) =>
        response
          .writeHead(200, { "content-type": "application/json" })
          .end(JSON.stringify(value));
      if (url.pathname === prefix) json({ default_branch: "proof/fixed" });
      else if (url.pathname === prefix + "/commits") {
        assert.equal(url.searchParams.get("sha"), "proof/fixed");
        json([{ sha: commit }]);
      } else {
        assert.equal(url.pathname, prefix + "/zipball/" + commit);
        response
          .writeHead(200, { "content-type": "application/zip" })
          .end(archive);
      }
      return true;
    },
    async verify() {
      assert.equal(requests.length, 3);
      await atomic("repository.json", {
        commit,
        content,
        source: "Actual owned Git commit; synthetic GitHub API boundary",
        requests,
      });
    },
  };
};
