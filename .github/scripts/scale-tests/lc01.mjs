import { closeSync, openSync, writeSync } from 'node:fs';
import { readFile, stat, writeFile } from 'node:fs/promises';
import { check, safeFailure } from './lib/checks.mjs';
import { validateConfig } from './lib/lc01-plan.mjs';
import { runCapacity } from './lib/lc01-run.mjs';
import { scoreCapacity } from './lib/lc01-score.mjs';

function privateOutput(path) {
  return openSync(path, 'wx', 0o600);
}

function junit(report) {
  const pass = report.status === 'PASS';
  const blocked = report.status === 'BLOCKED';
  const outcome = pass ? '' : blocked ? '<skipped message="capacity evidence incomplete"/>'
    : '<failure message="capacity threshold or correctness failure"/>';
  return `<?xml version="1.0" encoding="UTF-8"?>\n`
    + `<testsuite name="CMA LC-01 capacity" tests="1" failures="${!pass && !blocked ? 1 : 0}" `
    + `skipped="${blocked ? 1 : 0}"><testcase name="TC-073">${outcome}</testcase></testsuite>\n`;
}

const [action, ...args] = process.argv.slice(2);
if (action === '--help') {
  console.log('node lc01.mjs run PRIVATE_MANIFEST RAW_NDJSON');
  console.log('node lc01.mjs score REPORT_JSON REPORT_JUNIT RAW_NDJSON OBSERVATIONS_JSON [RAW_NDJSON OBSERVATIONS_JSON ...]');
} else {
  try {
    if (action === 'run') {
      check(args.length === 2, 'capacity_arguments_invalid', true);
      const [manifest, raw] = args;
      const metadata = await stat(manifest);
      check(metadata.isFile() && (metadata.mode & 0o077) === 0, 'private_manifest_required', true);
      const config = validateConfig(JSON.parse(await readFile(manifest, 'utf8')));
      const out = privateOutput(raw);
      const controller = new AbortController();
      const stop = () => controller.abort();
      process.once('SIGINT', stop);
      process.once('SIGTERM', stop);
      try {
        await runCapacity(config, (row) => { writeSync(out, JSON.stringify(row) + '\n'); }, controller.signal);
      } catch (error) {
        writeSync(out, JSON.stringify({ type: 'aborted', ...safeFailure(error) }) + '\n');
        throw error;
      } finally {
        closeSync(out);
      }
      console.log('Client capture complete. Score it with independent staging observations.');
    } else if (action === 'score') {
      check(args.length >= 4 && args.length % 2 === 0, 'capacity_arguments_invalid', true);
      const [reportPath, junitPath, ...inputs] = args;
      let report;
      try {
        const captures = [];
        for (let i = 0; i < inputs.length; i += 2) {
          const raw = await readFile(inputs[i], 'utf8');
          const evidence = JSON.parse(await readFile(inputs[i + 1], 'utf8'));
          const rows = raw.trimEnd().split('\n').map((line) => JSON.parse(line));
          captures.push({ rows, evidence });
        }
        report = scoreCapacity(captures);
      } catch (error) {
        report = { schema: 'cma-lc01-score-v1', ...safeFailure(error) };
      }
      await writeFile(reportPath, JSON.stringify(report, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
      await writeFile(junitPath, junit(report), { flag: 'wx', mode: 0o600 });
      console.log(`Capacity score ${report.status}`);
      process.exitCode = report.status === 'PASS' ? 0 : 2;
    } else {
      check(false, 'capacity_arguments_invalid', true);
    }
  } catch (error) {
    console.error(safeFailure(error).code);
    process.exitCode = 2;
  }
}
