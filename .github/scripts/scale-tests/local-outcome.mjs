import { readFile } from 'node:fs/promises';
import { check, safeFailure } from './lib/checks.mjs';
import { verifyLocalOutcomes } from './lib/local-outcome-oracle.mjs';

try {
  check(process.argv.length === 4 && process.argv[2] === '--config',
    'oracle_config_required', true);
  const config = JSON.parse(await readFile(process.argv[3], 'utf8'));
  const result = await verifyLocalOutcomes(config);
  console.log(JSON.stringify({ status: 'PASS', ...result }));
} catch (error) {
  console.error(JSON.stringify(safeFailure(error)));
  process.exitCode = 2;
}
