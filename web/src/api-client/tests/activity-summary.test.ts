// @vitest-environment node
import { describe, expect, it } from 'vitest';
import { Administration } from '../admin/index.js';
import { ApiError, ProtocolError, ValidationError } from '../errors.js';
import { now, transport } from './resource-fixtures.js';

const row = {
  id: 'act-fixture',
  at: now,
  actor: null,
  target: null,
  action: 'gateway_record_read',
  outcome: { status: 'succeeded' },
};
describe('Activity summary contract', () => {
  it('requests explicit summary fields and preserves the full-record method', async () => {
    const t = transport();
    const admin = new Administration(t.client.config);
    t.answer({ items: [row], next: '123:act_next' });
    expect(
      await admin.listActivitySummary({
        limit: 50,
        cursor: '456:act_before',
        action: 'gateway_record_read',
      }),
    ).toEqual({ items: [row], next: '123:act_next' });
    expect(Object.fromEntries(new URL(t.calls[0]!.url).searchParams)).toEqual({
      view: 'summary',
      limit: '50',
      cursor: '456:act_before',
      action: 'gateway_record_read',
    });
    const complete = { ...row, detail: { resource_name: 'retained detail' } };
    t.answer({ items: [complete], next: null });
    expect((await admin.listActivity()).items).toEqual([complete]);
    expect(new URL(t.calls[1]!.url).searchParams.has('view')).toBe(false);
  });

  it('rejects invalid input and malformed displayed fields', async () => {
    const t = transport();
    const admin = new Administration(t.client.config);
    for (const limit of [0, 201, 1.5])
      expect(() => admin.listActivitySummary({ limit })).toThrow(ValidationError);
    expect(t.calls).toHaveLength(0);
    t.answer({ items: [{ ...row, outcome: { status: 'unknown' } }], next: null });
    await expect(admin.listActivitySummary()).rejects.toBeInstanceOf(ProtocolError);
  });

  it('opts into server-side exclusion without changing the ordinary request', async () => {
    const t = transport();
    const admin = new Administration(t.client.config);
    t.answer({ items: [], next: null });
    await admin.listActivitySummary({ excludeSuccessfulReads: true, limit: 50 });
    const params = new URL(t.calls[0]!.url).searchParams;
    expect(params.get('exclude_successful_reads')).toBe('true');
    expect(params.get('view')).toBe('summary');
  });

  it('propagates authorization refusal without a second fallback request', async () => {
    const t = transport();
    const admin = new Administration(t.client.config);
    t.answer({ title: 'Forbidden', status: 403, code: 'forbidden' }, 403);
    await expect(
      admin.listActivitySummary({ excludeSuccessfulReads: true }),
    ).rejects.toBeInstanceOf(ApiError);
    expect(t.calls).toHaveLength(1);
  });

  it('retries an old server that refuses the new option', async () => {
    const t = transport();
    const admin = new Administration(t.client.config);
    t.fetch.mockImplementationOnce(async (url, init) => {
      t.calls.push({ url: String(url), init: init ?? {} });
      return Response.json(
        { title: 'Invalid request', status: 400, code: 'invalid_admin_request' },
        { status: 400 },
      );
    });
    t.answer({ items: [row], next: null });
    expect((await admin.listActivitySummary({ excludeSuccessfulReads: true })).items).toEqual([
      row,
    ]);
    expect(new URL(t.calls[0]!.url).searchParams.get('exclude_successful_reads')).toBe('true');
    expect(new URL(t.calls[1]!.url).searchParams.has('exclude_successful_reads')).toBe(false);
  });
});
