import { readFile } from 'node:fs/promises';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { check, safeFailure } from './lib/checks.mjs';
import { pinnedModule, validateConfig, guardedFetch } from './lib/safety.mjs';
import { selections, selectedScenarioNames, saveReport, saveSamples, sw04Scenario, sw04Selection } from './lib/report.mjs';
import { runSuite } from './lib/suite.mjs';
import { runSw04Fixture } from './lib/sw04-cancel.mjs';

const args = process.argv.slice(2);
let output = resolve('results', new Date().toISOString().replaceAll(':', '-'));
let scenario = 'all';
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
  if (scenario === sw04Selection) {
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
  const scoped = scenario === sw04Selection;
  const report = { schema: 'cma-scale-results-v1', environment: 'local', status: 'BLOCKED',
    selection: scoped || selections.includes(scenario) ? scenario : 'all',
    started_at: new Date().toISOString(), scenarios: (scoped ? [sw04Scenario] : selectedScenarioNames(scenario)).map((scenario) => ({ scenario,
      status: 'BLOCKED', code: failure.code })) };
  if (scoped) report.track_status = 'INCOMPLETE';
  await saveReport(output, report);
  console.error(`BLOCKED: ${failure.code}; evidence=${output}`);
  process.exitCode = 2;
}
