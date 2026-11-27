import { mkdir, readFile } from 'node:fs/promises';
import { dirname, resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { check, GateError, hash, safeFailure } from './lib/checks.mjs';
import { pinnedModule, validateConfig, guardedFetch } from './lib/safety.mjs';
import { selections, selectedScenarioNames, saveReport, saveSamples, sw04Scenario, sw04Selection,
  connectedWorkloadScenario, connectedWorkloadSelection } from './lib/report.mjs';
import { runSuite } from './lib/suite.mjs';
import { runSw04Fixture } from './lib/sw04-cancel.mjs';
import { runLocalWorkload } from './lib/local-workload.mjs';

const args = process.argv.slice(2);
let output = resolve('results', new Date().toISOString().replaceAll(':', '-'));
let scenario = 'all';
let connectedOutputReserved = false;
try {
  check(args.length % 2 === 0, 'arguments_require_values', true);
  const options = {};
  for (let i = 0; i < args.length; i += 2) {
    check(['--config', '--out', '--scenario', '--observations'].includes(args[i])
      && !options[args[i]], 'invalid_argument', true);
    options[args[i]] = args[i + 1];
  }
  if (options['--out']) output = resolve(options['--out']);
  scenario = options['--scenario'] ?? 'all';
  if (scenario === connectedWorkloadSelection) {
    check(!options['--observations'], 'unexpected_observations_path', true);
    check(options['--config'], 'environment_config_missing', true);
    const config = JSON.parse(await readFile(options['--config'], 'utf8'));
    await mkdir(dirname(output), { recursive: true, mode: 0o700 });
    await mkdir(output, { mode: 0o700 });
    connectedOutputReserved = true;
    const controller = new AbortController();
    const cancel = () => controller.abort(new GateError('local_workload_interrupted'));
    process.once('SIGINT', cancel);
    process.once('SIGTERM', cancel);
    try {
      const evidence = await runLocalWorkload(config, { signal: controller.signal });
      const row = { scenario: connectedWorkloadScenario, status: evidence.status,
        code: evidence.code, evidence_hash: hash(JSON.stringify(evidence)), evidence };
      await saveReport(output, { schema: 'cma-scale-results-v1', environment: 'local', selection: scenario,
        started_at: evidence.startedAt ?? evidence.finishedAt, status: 'NO_GO', track_status: 'INCOMPLETE', scenarios: [row] });
      console.log(`${row.status === 'PASS' ? 'DONE' : 'BLOCKED'}: connected workload ${row.status}; evidence=${output}`);
      process.exitCode = row.status === 'PASS' ? 0 : 2;
    } finally {
      process.off('SIGINT', cancel);
      process.off('SIGTERM', cancel);
    }
  } else if (scenario === sw04Selection) {
    check(!options['--config'], 'unexpected_environment_config', true);
    check(options['--observations'], 'sw04_observations_path_missing', true);
    const report = runSw04Fixture(await readFile(options['--observations'], 'utf8'));
    await saveReport(output, report);
    const [result] = report.scenarios;
    console.log(`${result.status === 'PASS' ? 'DONE' : 'BLOCKED'}: SW04 Cancel ${result.status}; evidence=${output}`);
    process.exitCode = result.status === 'PASS' ? 0 : 2;
  } else {
    check(selections.includes(scenario), 'invalid_scenario', true);
    check(!options['--observations'], 'unexpected_observations_path', true);
    check(options['--config'], 'environment_config_missing', true);
    const config = validateConfig(JSON.parse(await readFile(options['--config'], 'utf8')));
    check(config.driverModule && config.driverSha256, 'environment_driver_missing', true);
    const module = await pinnedModule(pathToFileURL(resolve(config.driverModule)), config.driverSha256);
    const driver = await module.createDriver(config, { fetch: guardedFetch() });
    const report = await runSuite(config, driver, { scenario, progress: (line) => console.log(line),
      checkpoint: (value) => saveReport(resolve(output, 'preregistered-noise'), value),
      samples: (cohort, index) => saveSamples(resolve(output, 'samples'), cohort, index) });
    await saveReport(output, report);
    console.log(`${report.status === 'GO' ? 'DONE' : 'BLOCKED'}: local suite ${report.status}; evidence=${output}`);
    process.exitCode = report.status === 'GO' ? 0 : 2;
  }
} catch (error) {
  const failure = safeFailure(error);
  const connected = scenario === connectedWorkloadSelection;
  const scoped = scenario === sw04Selection || connected;
  const names = connected ? [connectedWorkloadScenario] : scoped ? [sw04Scenario] : selectedScenarioNames(scenario);
  const report = { schema: 'cma-scale-results-v1', environment: 'local', status: 'BLOCKED',
    selection: scoped || selections.includes(scenario) ? scenario : 'all',
    started_at: new Date().toISOString(), scenarios: names.map((scenario) => ({ scenario,
      status: 'BLOCKED', code: failure.code })) };
  if (scoped) report.track_status = 'INCOMPLETE';
  if (!connected || connectedOutputReserved) await saveReport(output, report);
  console.error(`BLOCKED: ${failure.code}; evidence=${output}`);
  process.exitCode = 2;
}
