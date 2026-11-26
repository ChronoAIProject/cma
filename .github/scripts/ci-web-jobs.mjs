// ci-web.yml runs the Web track as parallel jobs. A contract that read the
// former single job now reads the one job that runs its step, so the step's
// prerequisites and failure evidence must stay in that job. Workflow-wide
// facts, such as which guard suites run, are still read from the whole file.
import assert from "node:assert/strict";

/** Raw text of each job under `jobs:`, keyed by job id. */
export function jobTexts(workflow) {
  const body = workflow.split(/^jobs:\n/m)[1];
  assert.ok(body, "Web workflow has no jobs");
  return Object.fromEntries(
    body
      .split(/^(?=  [A-Za-z0-9_-]+:\n)/m)
      .filter(Boolean)
      .map((text) => [text.match(/^  ([A-Za-z0-9_-]+):\n/)[1], text]),
  );
}

/** Raw text of the one job that runs the step named `name`. */
export function jobRunning(workflow, name) {
  const marker = `name: "${name}"\n`;
  const holders = Object.values(jobTexts(workflow)).filter((text) =>
    text.includes(marker),
  );
  assert.equal(holders.length, 1, `exactly one Web job must run "${name}"`);
  return holders[0];
}

/** The one parsed job with a step matching `predicate`. */
export function jobWith(workflow, predicate, label) {
  const holders = Object.values(workflow.jobs).filter((job) =>
    job.steps.some(predicate),
  );
  assert.equal(holders.length, 1, `exactly one Web job must run ${label}`);
  return holders[0];
}
