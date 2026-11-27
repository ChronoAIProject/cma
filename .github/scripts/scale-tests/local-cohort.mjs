import { readFile } from 'node:fs/promises';
import { check, safeFailure } from './lib/checks.mjs';
import { verifyLocalCohort } from './lib/local-cohort.mjs';

try {
  check(process.argv.length === 4 && process.argv[2] === '--config',
    'local_cohort_config_required', true);
  const config = JSON.parse(await readFile(process.argv[3], 'utf8'));
  const evidence = await verifyLocalCohort(config);
  console.log(JSON.stringify({ status: 'PASS', boundary: 'initial_cohort', ...evidence }));
} catch (error) {
  console.error(JSON.stringify(safeFailure(error)));
  process.exitCode = 2;
}
