// Usage: node check-defaults.mjs PRIVATE_NATIVE_OBSERVATION [PRIVATE_APPROVED_VALUATION]
// Exit 0 = component defaults match; 1 = mismatch; 2 = missing/invalid evidence.
// No result from this command constitutes M-SUST-05 runtime acceptance.
import {readFile} from 'node:fs/promises';
import {assessDefaults} from './policy.mjs';

try {
  const [observationPath, valuationPath, extra] = process.argv.slice(2);
  if (!observationPath || extra) throw Error('expected observation and optional valuation paths');
  const observation = JSON.parse(await readFile(observationPath, 'utf8'));
  if (observation.scope !== 'NATIVE_CORE_DEFAULTS_ONLY' || observation.kind_runtime !== false)
    throw Error('expected native component observation; runtime qualification is separate');
  const valuation = valuationPath ? JSON.parse(await readFile(valuationPath, 'utf8')) : undefined;
  const result = assessDefaults(observation, valuation);
  console.log(JSON.stringify(result, null, 2));
  process.exitCode = result.status === 'PASS' ? 0 : result.status === 'FAIL' ? 1 : 2;
} catch {
  // Input evidence may contain private policy data; do not echo it on errors.
  console.error('Cost defaults evidence is missing or invalid.');
  process.exitCode = 2;
}
