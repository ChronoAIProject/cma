import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";
import { auditWeb, npmJson } from "./audit-dependencies.mjs";
import { evaluateAudit } from "./audit-policy.mjs";

const id = "GHSA-fc5q-39xc-vjpf";
const today = "2026-11-18";
const exception = {
  advisory: id,
  package: "picomatch",
  scope: "dev-only",
  reason: "No patched release; build tooling only.",
  expires: "2026-11-25",
};
const production = {
  name: "example-web",
  dependencies: { react: { version: "1.0.0" } },
};
const advisory = (name = "picomatch", severity = "high", advisory = id) => ({
  name,
  dependency: name,
  severity,
  url: `https://github.com/advisories/${advisory}`,
});
const vulnerability = (name, via, severity = "high") => ({
  name,
  via,
  severity,
});
function report() {
  return {
    auditReportVersion: 2,
    vulnerabilities: {
      picomatch: vulnerability("picomatch", [advisory()]),
      tinyglobby: vulnerability("tinyglobby", ["picomatch"]),
      vite: vulnerability("vite", ["tinyglobby"]),
      vitest: vulnerability("vitest", ["coverage", "tinyglobby"]),
      coverage: vulnerability("coverage", ["vitest"]),
    },
  };
}
const evaluate = (
  audit = report(),
  tree = production,
  entries = [exception],
  date = today,
) => evaluateAudit(audit, tree, entries, date);

test("accept only the dev advisory and its npm parent/cyclic peer effects", () => {
  assert.deepEqual(evaluate(), [exception]);
});

test("a clean audit needs no exception", () => {
  assert.deepEqual(
    evaluate({ auditReportVersion: 2, vulnerabilities: {} }, production, []),
    [],
  );
});

test("unrelated high and critical advisories fail even beside an excepted source", () => {
  for (const severity of ["high", "critical"]) {
    const audit = report();
    audit.vulnerabilities.vite.via.push(
      advisory("vite", severity, "GHSA-xxxx-xxxx-xxxx"),
    );
    assert.throws(() => evaluate(audit), /Unexcepted high\/critical.*vite/);
  }
});

test("a second advisory on the excepted package fails", () => {
  const audit = report();
  audit.vulnerabilities.picomatch.via.push(
    advisory("picomatch", "high", "GHSA-xxxx-xxxx-xxxx"),
  );
  assert.throws(() => evaluate(audit), /GHSA-xxxx-xxxx-xxxx/);
});

test("an unidentified severe advisory is never excepted", () => {
  const audit = report();
  audit.vulnerabilities.picomatch.via[0].url =
    "https://example.invalid/advisory";
  assert.throws(() => evaluate(audit), /unidentified advisory/);
});

test("moderate advisories retain the existing high threshold", () => {
  const audit = report();
  audit.vulnerabilities.minimatch = vulnerability(
    "minimatch",
    [advisory("minimatch", "moderate", "GHSA-xxxx-xxxx-xxxx")],
    "moderate",
  );
  assert.deepEqual(evaluate(audit), [exception]);
});

test("excepted packages at any production dependency depth fail", () => {
  for (const tree of [
    { dependencies: { picomatch: { version: "4.0.7" } } },
    {
      dependencies: {
        wrapper: { dependencies: { picomatch: { version: "4.0.7" } } },
      },
    },
    { dependencies: { alias: { name: "picomatch", version: "4.0.7" } } },
  ])
    assert.throws(
      () => evaluate(report(), tree),
      /in the production tree: picomatch/,
    );
});

test("expiry includes its UTC date and fails the next date", () => {
  assert.deepEqual(evaluate(report(), production, [exception], "2026-11-25"), [
    exception,
  ]);
  assert.throws(
    () => evaluate(report(), production, [exception], "2026-11-26"),
    /exception expired/,
  );
});

test("missing and downgraded advisories make an exception stale", () => {
  assert.throws(
    () => evaluate({ auditReportVersion: 2, vulnerabilities: {} }),
    /Unused audit exception/,
  );
  const audit = {
    auditReportVersion: 2,
    vulnerabilities: {
      picomatch: vulnerability(
        "picomatch",
        [advisory("picomatch", "moderate")],
        "moderate",
      ),
    },
  };
  assert.throws(() => evaluate(audit), /Unused audit exception/);
});

test("advisory id cannot except a different package", () => {
  const audit = report();
  audit.vulnerabilities.picomatch.via[0].dependency = "another-package";
  assert.throws(() => evaluate(audit), /Unexcepted high\/critical/);
  assert.throws(
    () => evaluate(report(), production, [{ ...exception, package: "other" }]),
    /Unexcepted high\/critical/,
  );
});

test("schema, dates, scopes and duplicate exceptions fail closed", () => {
  for (const entries of [
    null,
    [null],
    [{ ...exception, extra: true }],
    [{ ...exception, advisory: "invalid" }],
    [{ ...exception, package: "../picomatch" }],
    [{ ...exception, scope: "all" }],
    [{ ...exception, reason: " " }],
    [{ ...exception, expires: "2026-02-30" }],
    [{ ...exception, expires: "soon" }],
    [exception, exception],
  ]) {
    assert.throws(() => evaluate(report(), production, entries));
  }
  assert.throws(
    () => evaluate(report(), production, [exception], "invalid"),
    /Invalid audit date/,
  );
});

test("missing or invalid production evidence fails closed", () => {
  for (const tree of [
    null,
    { error: {} },
    { missing: true },
    { invalid: true },
    { problems: ["missing dependency"] },
    { dependencies: [] },
    { dependencies: { wrapper: null } },
  ]) {
    assert.throws(() => evaluate(report(), tree), /production|Production/);
  }
});

test("audit errors, malformed entries and dangling graph edges fail closed", () => {
  for (const audit of [
    null,
    {},
    { auditReportVersion: 2, error: {}, vulnerabilities: {} },
    { auditReportVersion: 2, vulnerabilities: { bad: {} } },
    {
      auditReportVersion: 2,
      vulnerabilities: { bad: vulnerability("bad", ["missing"]) },
    },
    {
      auditReportVersion: 2,
      vulnerabilities: { bad: vulnerability("bad", [null]) },
    },
  ]) {
    assert.throws(() => evaluate(audit));
  }
});

test("severe parents without concrete severe advisory sources fail", () => {
  const audit = report();
  audit.vulnerabilities.other = vulnerability("other", ["other"]);
  assert.throws(
    () => evaluate(audit),
    /Cannot establish severe advisory sources for other/,
  );
});

test("npm command failures, signals and malformed JSON fail closed", () => {
  for (const result of [
    { status: 2 },
    { status: 0, error: new Error("offline") },
    { status: null, signal: "SIGTERM" },
  ]) {
    assert.throws(
      () => npmJson(["audit"], ".", () => result),
      /evidence is unavailable/,
    );
  }
  assert.throws(
    () => npmJson(["ls"], ".", () => ({ status: 1, stdout: "{}" })),
    /evidence is unavailable/,
  );
  assert.throws(
    () => npmJson(["audit"], ".", () => ({ status: 0, stdout: "not json" })),
    SyntaxError,
  );
  assert.deepEqual(
    npmJson(["audit"], ".", () => ({ status: 1, stdout: "{}" })),
    {},
  );
});

test("runner obtains fresh full audit and production-only dependency tree", (t) => {
  const root = mkdtempSync(join(tmpdir(), "cma-audit-"));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  mkdirSync(join(root, "web"));
  // Runner time is real; policy expiry boundaries use the injected test date.
  writeFileSync(
    join(root, "web/audit-exceptions.json"),
    JSON.stringify([{ ...exception, expires: "2099-01-01" }]),
  );
  const calls = [];
  const run = (command, args, options) => {
    calls.push(args);
    assert.equal(command, "npm");
    assert.equal(options.cwd, join(root, "web"));
    assert.ok(options.timeout > 0);
    return {
      status: args[0] === "audit" ? 1 : 0,
      stdout: JSON.stringify(args[0] === "audit" ? report() : production),
    };
  };
  auditWeb(root, run);
  assert.deepEqual(calls, [
    ["ls", "--omit=dev", "--all", "--json"],
    ["audit", "--audit-level=high", "--json"],
  ]);
  writeFileSync(join(root, "web/audit-exceptions.json"), "invalid");
  assert.throws(() => auditWeb(root, run), SyntaxError);
});
