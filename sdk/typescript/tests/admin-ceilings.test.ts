import { expect, it } from 'vitest';
import { ValidationError } from '../src/index.js';
import { transport } from './resource-fixtures.js';

const ceilings = {
  registrations: 10,
  retained_homes: 5,
  active_sessions: 4,
  owner_active_sessions: 1,
  existing_reserve: { retained_homes: 2, active_sessions: 1 },
};
it('replaces capacity ceilings with independent revision CAS without retrying', async () => {
  const t = transport();
  const state = { revision: 1, ceilings, last_change: null, writes_available: true };
  t.answer(state);
  const request = { ceilings, reason: 'bound public capacity', expected_revision: 0 };
  expect(await t.client.admin.replaceAdmissionCeilings(request)).toEqual(state);
  expect(new URL(t.calls[0]!.url).pathname).toBe('/api/v1/admin/admission/ceilings');
  expect(t.calls[0]!.init.method).toBe('PUT');
  expect(JSON.parse(String(t.calls[0]!.init.body))).toEqual(request);
  expect(t.calls).toHaveLength(1);
});
it.each([
  { ...ceilings, active_sessions: -1 },
  { ...ceilings, active_sessions: 0x100000000 },
  { ...ceilings, existing_reserve: { retained_homes: 6, active_sessions: 1 } },
])('rejects invalid ceilings before IO', (invalid) => {
  const t = transport();
  expect(() =>
    t.client.admin.replaceAdmissionCeilings({
      ceilings: invalid,
      reason: 'invalid',
      expected_revision: 0,
    }),
  ).toThrow(ValidationError);
  expect(t.calls).toHaveLength(0);
});
it('preserves capacity observations and separate newest-first history', async () => {
  const t = transport();
  const state = { revision: 0, ceilings, last_change: null, writes_available: false };
  const observation = { observed: 3, bound: 4, saturated: false };
  const view = {
    revision: 0,
    operator_stop: null,
    last_change: null,
    writes_available: false,
    history: [],
    ceilings: state,
    effective_limits: ceilings,
    capacity: {
      registrations: null,
      retained_homes: null,
      pending_launch_homes: 9,
      active_sessions: observation,
    },
    ceilings_history: [],
  };
  t.answer(view);
  expect(await t.client.admin.readAdmission()).toEqual(view);
});

it('accepts an effective deployment home cap below the configured reserve', async () => {
  const t = transport();
  const policy = {
    ...ceilings,
    retained_homes: 100,
    existing_reserve: { retained_homes: 80, active_sessions: 1 },
  };
  const effective = { ...policy, retained_homes: 10 };
  const view = {
    revision: 0,
    operator_stop: null,
    last_change: null,
    writes_available: false,
    history: [],
    ceilings: { revision: 1, ceilings: policy, last_change: null, writes_available: true },
    effective_limits: effective,
    capacity: { registrations: null, retained_homes: null, active_sessions: null },
    ceilings_history: [],
  };
  t.answer(view);
  expect((await t.client.admin.readAdmission()).effective_limits).toEqual(effective);
});
