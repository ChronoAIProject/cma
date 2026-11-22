// @vitest-environment node
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { expect, it } from 'vitest';
import { Journal } from './journal';

it('records one scoped request identity across repeated pending facts without erasing history', () => {
  const output = mkdtempSync(join(tmpdir(), 'cma-external-cleanup-'));
  try {
    const journal = new Journal(output, 'current-run');
    const first = journal.requireExternalAccountCleanup(
      'https://staging.example.test',
      'fixture-owner',
      'new-subject',
    );
    expect(
      journal.requireExternalAccountCleanup(
        'https://staging.example.test',
        'fixture-owner',
        'new-subject',
      ),
    ).toBe(first);
    expect(
      journal.requireExternalAccountCleanup(
        'https://staging.example.test',
        'fixture-owner',
        'other-subject',
      ),
    ).not.toBe(first);
    expect(
      journal.requireExternalAccountCleanup(
        'https://other.example.test',
        'fixture-owner',
        'new-subject',
      ),
    ).not.toBe(first);
    const next = new Journal(output, 'next-run');
    expect(
      next.requireExternalAccountCleanup(
        'https://staging.example.test',
        'fixture-owner',
        'new-subject',
      ),
    ).not.toBe(first);
    const rows = readFileSync(join(output, 'cleanup.ndjson'), 'utf8')
      .trim()
      .split('\n')
      .map((line) => JSON.parse(line));
    expect(rows).toHaveLength(5);
    expect(rows.slice(0, 2)).toEqual([
      expect.objectContaining({
        run: 'current-run',
        requestId: first,
        action: 'external_account_cleanup_required',
        subject: 'new-subject',
      }),
      expect.objectContaining({
        run: 'current-run',
        requestId: first,
        action: 'external_account_cleanup_required',
        subject: 'new-subject',
      }),
    ]);
    expect(rows.every((row) => row.schema === 'cma-external-cleanup-request-v1')).toBe(true);
    expect(rows.every((row) => /^[a-f0-9]{64}$/.test(row.originHash))).toBe(true);
    expect(journal.resources).toEqual([]);
    expect(() =>
      journal.requireExternalAccountCleanup('https://staging.example.test', 'fixture-owner', ''),
    ).toThrow();
  } finally {
    rmSync(output, { recursive: true });
  }
});
