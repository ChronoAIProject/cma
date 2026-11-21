import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

// Register a suite with one {"prefix":"module::tests::"} JSON file in
// mongo-contract-suites/. Source/archive fixtures share the runner's parser.
export const requiredPrefixes = JSON.parse(execFileSync('python3', [
  fileURLToPath(new URL('mongo-contract-registry.py', import.meta.url)), '--prefixes',
], {encoding: 'utf8'}));
