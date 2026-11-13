import { expect, it } from 'vitest';
import { sandbox, transport } from './resource-fixtures.js';

it.each([true, false])(
  'sends the explicit administrator exception filter %s',
  async (always_on) => {
    const t = transport();
    t.answer({ items: [], next: null });
    await t.client.admin.listSandboxes({ always_on, limit: 25, cursor: 'sbx_after' });
    const url = new URL(t.calls[0]!.url, 'https://example.invalid');
    expect(url.searchParams.get('always_on')).toBe(String(always_on));
    expect(url.searchParams.get('limit')).toBe('25');
    expect(url.searchParams.get('cursor')).toBe('sbx_after');
  },
);

it('preserves the typed owner refusal without retrying it', async () => {
  const t = transport();
  t.answer(
    { code: 'always_on_approval_required', detail: 'An administrator must approve Always on.' },
    403,
  );
  await expect(
    t.client.sandboxes.setAlwaysOn(
      sandbox.id,
      { always_on: true, expected_revision: 1 },
      { idempotencyKey: 'request-approval' },
    ),
  ).rejects.toMatchObject({ status: 403, code: 'always_on_approval_required', isRetryable: false });
  expect(t.calls).toHaveLength(1);
});

it('sends administrator approval and revocation with the supplied revision', async () => {
  const t = transport();
  for (const always_on of [true, false]) {
    t.answer({ ...sandbox, always_on, revision: 2 });
    const result = await t.client.admin.setSandboxAlwaysOn(sandbox.id, {
      always_on,
      expected_revision: 1,
    });
    expect(result.always_on).toBe(always_on);
    expect(JSON.parse(String(t.calls.at(-1)!.init.body))).toEqual({
      always_on,
      expected_revision: 1,
    });
  }
});
