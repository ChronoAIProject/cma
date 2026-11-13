import { expect, it } from 'vitest';
import { ApiError, ProtocolError, ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

const open = { revision: 0, operator_stop: null, last_change: null, writes_available: false };
const stop = { since: 100, revision: 1, actor: 'operator-a', reason: 'contain incident' };
const stopped = {
  revision: 1,
  operator_stop: stop,
  last_change: {
    revision: 1,
    at: 100,
    actor: 'operator-a',
    reason: 'contain incident',
    old_stop: null,
    new_stop: stop,
  },
  writes_available: true,
};

it('reads independent admission state and sends explicit stop/resume CAS without retries', async () => {
  const t = transport();
  t.answer({ ...open, history: [] });
  expect(await t.client.admin.readAdmission()).toEqual({ ...open, history: [] });
  expect(new URL(t.calls[0]!.url).pathname).toBe('/api/v1/admin/admission');
  t.answer(stopped);
  const request = { reason: 'contain incident', expected_revision: 0 };
  expect(await t.client.admin.stopAdmission(request)).toEqual(stopped);
  expect(new URL(t.calls[1]!.url).pathname).toBe('/api/v1/admin/admission/stop');
  expect(t.calls[1]!.init.method).toBe('POST');
  expect(JSON.parse(String(t.calls[1]!.init.body))).toEqual(request);
  t.answer({ ...open, revision: 2, writes_available: true });
  await t.client.admin.resumeAdmission({ reason: 'resolved', expected_revision: 1 });
  expect(new URL(t.calls[2]!.url).pathname).toBe('/api/v1/admin/admission/resume');
  expect(t.calls).toHaveLength(3);
});

it.each(['', ' ', 'bad\nreason', 'a'.repeat(513)])(
  'rejects invalid reasons before sending',
  (reason) => {
    const t = transport();
    expect(() => t.client.admin.stopAdmission({ reason, expected_revision: 0 })).toThrow(
      ValidationError,
    );
    expect(t.calls).toHaveLength(0);
  },
);

it('rejects malformed admission reads and preserves typed closed-gate errors', async () => {
  const t = transport();
  t.answer({ ...open, writes_available: 'true', history: [] });
  await expect(t.client.admin.readAdmission()).rejects.toBeInstanceOf(ProtocolError);
  t.answer(
    {
      type: 'urn:cma:admission-control-writes-unavailable',
      title: 'Unavailable',
      status: 503,
      detail: 'Saving admission control is not available yet.',
      code: 'admission_control_writes_unavailable',
    },
    503,
  );
  await expect(
    t.client.admin.stopAdmission({ reason: 'incident', expected_revision: 0 }),
  ).rejects.toBeInstanceOf(ApiError);
  expect(t.calls).toHaveLength(2);
});

it('preserves full immutable read history while command responses keep their existing shape', async () => {
  const t = transport();
  const resumed = {
    revision: 2,
    at: 101,
    actor: 'operator-b',
    reason: 'resolved',
    old_stop: stop,
    new_stop: null,
  };
  const observed = {
    ...open,
    revision: 2,
    last_change: resumed,
    history: [resumed, stopped.last_change],
  };
  t.answer(observed);
  expect(await t.client.admin.readAdmission()).toEqual(observed);
  t.answer(stopped);
  expect(await t.client.admin.stopAdmission({ reason: 'incident', expected_revision: 0 })).toEqual(
    stopped,
  );
});

it.each([
  { history: undefined },
  { history: [{ ...stopped.last_change, revision: '1' }] },
  { history: [stopped.last_change, { ...stopped.last_change, revision: 2 }] },
  {
    history: Array.from({ length: 21 }, (_, index) => ({
      ...stopped.last_change,
      revision: 21 - index,
    })),
  },
])('rejects missing, malformed, unordered or unbounded admission history', async ({ history }) => {
  const t = transport();
  t.answer({ ...open, history });
  await expect(t.client.admin.readAdmission()).rejects.toBeInstanceOf(ProtocolError);
});
