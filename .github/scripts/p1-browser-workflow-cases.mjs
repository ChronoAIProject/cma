import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import test from "node:test";
import { jobWith } from "./ci-web-jobs.mjs";
import { suites } from "./p1-browser-ci.mjs";

const require = createRequire(new URL("../../web/package.json", import.meta.url));
const yaml = require("js-yaml");
const workflow = yaml.load(await readFile(new URL("../workflows/ci-web.yml", import.meta.url), "utf8"));

const designJob = (workflow) =>
  jobWith(workflow, (step) => step.name === "Design, shell and auth gate mechanics", "the design gate mechanics");
function verifyWorkflow(workflow) {
  const job = designJob(workflow);
  assert.equal(job.defaults.run["working-directory"], "web");
  assert.equal(job["continue-on-error"], undefined);
  const steps = job.steps;
  const everywhere = Object.values(workflow.jobs).flatMap((entry) => entry.steps);
  const mechanics = steps.filter((step) => step.name === "Design, shell and auth gate mechanics");
  assert.equal(mechanics.length, 1);
  assert.equal(mechanics[0].run, "node --test ../.github/scripts/p1-browser-ci.test.mjs");
  // The lanes serve this job's own production bundle in its own pinned browser.
  const build = steps.findIndex((s) => s.name === "Production build");
  const browser = steps.findIndex((s) => s.name === "Install pinned local browser");
  assert.ok(build >= 0 && browser >= 0 && build < steps.indexOf(mechanics[0]) && browser < steps.indexOf(mechanics[0]));
  // Every suite needs its own bounded step; a split suite cannot silently stop running.
  for (const suite of Object.keys(suites)) {
    const selected = steps.filter((step) => step.id === `p1_${suite}`);
    assert.equal(selected.length, 1);
    assert.equal(everywhere.filter((s) => s.id === `p1_${suite}`).length, 1);
    const step = selected[0];
    assert.equal(step.run, `node ../.github/scripts/p1-browser-ci.mjs ${suite}`);
    assert.ok(steps.indexOf(mechanics[0]) < steps.indexOf(step));
    // Parallel jobs have no order between them; within one job the lanes keep their place.
    const agent = steps.findIndex((s) => s.id === "agent_v2");
    const request = steps.findIndex((s) => s.name === "New agent request failure evidence");
    if (agent >= 0) assert.ok(steps.indexOf(step) < agent);
    if (request >= 0) assert.ok(steps.indexOf(step) > request);
    for (const checked of [step, mechanics[0]]) {
      for (const key of ["if", "continue-on-error", "env", "working-directory"])
        assert.equal(checked[key], undefined);
      assert.ok(checked["timeout-minutes"] > 0 && checked["timeout-minutes"] <= 10);
    }
    const artifact = steps[steps.indexOf(step) + 1];
    assert.equal(artifact.if, `failure() && steps.p1_${suite}.outcome == 'failure'`);
    assert.equal(artifact.uses, "actions/upload-artifact@cf430e030ddbb5b0abf93d22962f4752f3646cd9");
    assert.equal(artifact.with.path, "${{ runner.temp }}/cma-p1-browser/" + suite);
    assert.equal(artifact.with["if-no-files-found"], "error");
    assert.equal(artifact.with["retention-days"], 1);
  }
}
test("workflow executes the guard and all suites, fails closed and uploads matching evidence", () => {
  verifyWorkflow(workflow);
  for (const mutate of [
    (j) => (j.steps = j.steps.filter((s) => s.id !== "p1_shell")),
    (j, w) => w.jobs.web.steps.push(...j.steps.splice(j.steps.findIndex((s) => s.id === "p1_shell"), 2)),
    (j, w) => w.jobs.web.steps.push(structuredClone(j.steps.find((s) => s.id === "p1_auth"))),
    (j) => (j.steps = j.steps.filter((s) => s.name !== "Production build")),
    (j) => (j.steps = j.steps.filter((s) => s.name !== "Install pinned local browser")),
    (j) => (j.steps = j.steps.filter((s) => s.id !== "p1_palette")),
    (j) => (j.steps.find((s) => s.id === "p1_palette").run = "node ../.github/scripts/p1-browser-ci.mjs design"),
    (j) => (j.steps.find((s) => s.name === "Design, shell and auth gate mechanics").run = "true"),
    (j) => (j.steps.find((s) => s.id === "p1_design").run += " --list"),
    (j) => (j.steps.find((s) => s.id === "p1_design")["continue-on-error"] = true),
    (j) => (j.steps.find((s) => s.id === "p1_design").if = "false"),
    (j) => (j.steps.find((s) => s.name === "Design browser failure evidence").with.path = "wrong"),
    (j) => delete j.steps.find((s) => s.name === "Design browser failure evidence").with["if-no-files-found"],
  ]) {
    const copy = structuredClone(workflow);
    mutate(designJob(copy), copy);
    assert.throws(() => verifyWorkflow(copy));
  }
});
