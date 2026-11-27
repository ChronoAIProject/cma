// @vitest-environment node
import { expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../index.js';
import { transport } from './resource-fixtures.js';

it('accepts older replies and sends bounded pages with explicit uncertain cursors', async () => {
  const t = transport();
  t.answer({ candidates: 3 });
  expect(await t.client.admin.previewAutoPause()).toEqual({ candidates: 3 });
  t.answer({ candidates: 1, failed: 0, next: 'sbx_next', enabled: true, policy_revision: 9 });
  expect((await t.client.admin.previewAutoPause({ cursor: '', limit: 25 })).next).toBe('sbx_next');
  expect(new URL(t.calls[1]!.url).searchParams.get('cursor')).toBe('');
  t.answer({ scheduled: 0, already_satisfied: 1, skipped: 2, failed: 1, next: '' });
  expect((await t.client.admin.pauseIdleNow(true, { cursor: 'sbx_next', limit: 25 })).next).toBe(
    '',
  );
  expect(JSON.parse(String(t.calls[2]!.init.body))).toEqual({
    confirmed: true,
    cursor: 'sbx_next',
    limit: 25,
  });
  t.answer({ scheduled: 0, already_satisfied: 0, skipped: 0, next: null });
  expect((await t.client.admin.pauseIdleNow(true)).next).toBeNull();
});

it('preserves the refusal from each Sandbox preview', async () => {
  const t = transport();
  const refusals = [
    { sandbox_id: 'sbx_legacy', reason: 'legacy_log_expected_rows' },
    { sandbox_id: 'sbx_recent', reason: 'recent_agent_activity' },
  ];
  t.answer({ candidates: 0, refusals });
  expect((await t.client.admin.previewAutoPause()).refusals).toEqual(refusals);
});

it('rejects invalid limits before transport and malformed batch metadata without retry', async () => {
  const t = transport();
  expect(() => t.client.admin.previewAutoPause({ limit: 101 })).toThrow(ValidationError);
  expect(() => t.client.admin.pauseIdleNow(true, { limit: 0 })).toThrow(ValidationError);
  expect(t.calls).toHaveLength(0);
  t.answer({ candidates: 0, failed: 'unknown' });
  await expect(t.client.admin.previewAutoPause()).rejects.toBeInstanceOf(ProtocolError);
  expect(t.calls).toHaveLength(1);
});
