// @vitest-environment node
import { expect, it } from 'vitest';
import { TransportError, ProtocolError, ValidationError } from '../index.js';
import { transport } from './resource-fixtures.js';

const read = (pending_homes: unknown) => ({
  revision: 0,
  operator_stop: null,
  last_change: null,
  writes_available: false,
  history: [],
  capacity: {
    registrations: null,
    retained_homes: null,
    active_sessions: null,
    pending_launch_homes: 4,
    pending_homes,
  },
});
const command = { reason: 'Investigate pending reservations', observed_pending: 4 };
const result = {
  previous_pending_launch_homes: 4,
  expected_pending_launch_homes: 3,
  pending_launch_homes: 4,
  dry_run: true,
  corrected: false,
};

it.each(['disabled', 'capacity_inactive', 'index_missing', 'index_mismatch', 'unavailable'])(
  'preserves %s without inventing measurements',
  async (status) => {
    const t = transport();
    const view = read({ status });
    t.answer(view);
    expect(await t.client.admin.readAdmission()).toEqual(view);
  },
);
it.each([-1, 0, 2])('parses verified signed drift %s', async (drift) => {
  const t = transport();
  const view = read({
    status: 'verified',
    expected_pending_launch_homes: 4 - drift,
    pending_launch_homes_drift: drift,
  });
  t.answer(view);
  expect(await t.client.admin.readAdmission()).toEqual(view);
});
it.each([
  { status: 'verified' },
  { status: 'unknown' },
  { status: 'verified', expected_pending_launch_homes: -1, pending_launch_homes_drift: 1 },
  { status: 'verified', expected_pending_launch_homes: 0, pending_launch_homes_drift: 0.5 },
  {
    status: 'verified',
    expected_pending_launch_homes: 0,
    pending_launch_homes_drift: -Number.MAX_SAFE_INTEGER - 1,
  },
])('rejects malformed census evidence', async (observation) => {
  const t = transport();
  t.answer(read(observation));
  await expect(t.client.admin.readAdmission()).rejects.toBeInstanceOf(ProtocolError);
});
it('preserves omitted preview and explicitly applies only when requested', async () => {
  const t = transport();
  t.answer(result);
  expect(await t.client.admin.reconcilePendingHomes(command)).toEqual(result);
  expect(new URL(t.calls[0]!.url).pathname).toBe('/api/v1/admin/admission/pending-homes/reconcile');
  expect(t.calls[0]!.init.method).toBe('POST');
  expect(JSON.parse(String(t.calls[0]!.init.body))).toEqual(command);
  const saved = { ...result, pending_launch_homes: 3, dry_run: false, corrected: true };
  t.answer(saved);
  expect(await t.client.admin.reconcilePendingHomes({ ...command, dry_run: false })).toEqual(saved);
  expect(JSON.parse(String(t.calls[1]!.init.body)).dry_run).toBe(false);
  expect(t.calls).toHaveLength(2);
});
it.each([
  { ...command, reason: ' ' },
  { ...command, reason: 'line\nbreak' },
  { ...command, reason: '界'.repeat(171) },
  { ...command, observed_pending: -1 },
  { ...command, observed_pending: 0.5 },
  { ...command, observed_pending: Number.MAX_SAFE_INTEGER + 1 },
  { ...command, unexpected: true },
])('rejects invalid commands before transport', (body) => {
  const t = transport();
  expect(() => t.client.admin.reconcilePendingHomes(body)).toThrow(ValidationError);
  expect(t.calls).toHaveLength(0);
});
it.each([
  [401, 'unauthorized'],
  [403, 'forbidden'],
  [409, 'pending_homes_conflict'],
  [422, 'invalid_request'],
  [503, 'pending_homes_unavailable'],
  [503, 'admission_ceilings_writes_unavailable'],
] as const)('preserves %s %s without retry', async (status, code) => {
  const t = transport();
  t.answer(
    { type: `urn:cma:${code}`, title: 'Refused', status, detail: 'Operation unavailable', code },
    status,
  );
  await expect(t.client.admin.reconcilePendingHomes(command)).rejects.toMatchObject({
    status,
    code,
  });
  expect(t.calls).toHaveLength(1);
});
it('does not retry transport uncertainty or malformed success', async () => {
  const t = transport();
  t.fetch.mockRejectedValueOnce(new TypeError('connection lost'));
  await expect(t.client.admin.reconcilePendingHomes(command)).rejects.toBeInstanceOf(
    TransportError,
  );
  expect(t.fetch).toHaveBeenCalledTimes(1);
  t.answer({ ...result, pending_launch_homes: -1 });
  await expect(t.client.admin.reconcilePendingHomes(command)).rejects.toBeInstanceOf(ProtocolError);
  expect(t.fetch).toHaveBeenCalledTimes(2);
});
