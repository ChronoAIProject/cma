import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { migrationShimOffences } from '../test/migrationShim';

const SRC = resolve(process.cwd(), 'src');

describe('the migrated landing, operations, and environment pages', () => {
  it('uses no migration shim beyond the shared responsive exemptions', () => {
    const sheets = [
      join(SRC, 'landing', 'Landing.css'),
      join(SRC, 'operator', 'OperatorPage.css'),
      join(SRC, 'environment', 'EnvironmentProfiles.css'),
    ];

    expect(
      migrationShimOffences(sheets, ['--focus-ring', '--page-gutter', '--read-column']),
    ).toEqual([]);
  });
});
