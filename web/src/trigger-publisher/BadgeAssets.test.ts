import { readFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { describe, expect, it } from 'vitest';
import { LABELS } from './model';

describe('publisher badge assets', () => {
  it('ships all twelve server-generated label/theme goldens byte for byte', () => {
    for (const label of Object.keys(LABELS))
      for (const theme of ['auto', 'light', 'dark']) {
        const name = `${label}-${theme}-available.svg`;
        const directory = dirname(fileURLToPath(import.meta.url));
        const asset = readFileSync(resolve(directory, 'badges', name), 'utf8');
        const golden = readFileSync(
          resolve(
            directory,
            '../../../api/crates/layer1-public/tests/fixtures/trigger-badges',
            name,
          ),
          'utf8',
        );
        expect(asset).toBe(golden);
      }
  });
});
