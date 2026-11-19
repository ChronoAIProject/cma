// @vitest-environment node
import { expect, it } from 'vitest';
import {
  ApiError,
  ProtocolError,
  ValidationError,
  type SpendObservationRequest,
} from '../index.js';
import { transport } from './resource-fixtures.js';

const disabled = { metering_enabled: false, settings_revision: 3 };
const observation: SpendObservationRequest = {
  source: 'cloud',
  observed_through: '2026-01-02T00:00:00Z',
  units: 10,
};
const enabled = {
  metering_enabled: true,
  settings_revision: 3,
  levels: { warning: 80, stop: 100, disposable: 90 },
  committed: 20,
  ledger: {
    period_start: '2026-01-01T00:00:00Z',
    period_end: '2026-02-01T00:00:00Z',
    settings_revision: 3,
    spent: 10,
    reserved: 10,
    adjustments: 0,
    closures: [],
  },
};

it('preserves disabled metering and omitted optional fields', async () => {
  const t = transport();
  for (const view of [disabled, enabled]) {
    t.answer(view);
    expect(await t.client.admin.readSpend()).toEqual(view);
  }
  expect(new URL(t.calls[0]!.url).pathname).toBe('/api/v1/admin/spend');
  expect(t.calls[0]!.init.method).toBe('GET');
});

it('posts explicit observations and permits an identical manual retry', async () => {
  const t = transport();
  const saved = {
    ...enabled,
    ledger: {
      ...enabled.ledger,
      cloud: {
        observed_through: observation.observed_through,
        units: observation.units,
        recorded_at: observation.observed_through,
        settings_revision: 3,
      },
      evaluated_at: observation.observed_through,
    },
  };
  for (let index = 0; index < 2; index++) {
    t.answer(saved);
    expect(await t.client.admin.recordSpendObservation(observation)).toEqual(saved);
    expect(new URL(t.calls[index]!.url).pathname).toBe('/api/v1/admin/spend/observations');
    expect(t.calls[index]!.init.method).toBe('POST');
    expect(JSON.parse(String(t.calls[index]!.init.body))).toEqual(observation);
  }
  expect(t.calls).toHaveLength(2);
});

it.each(['spend_warning', 'spend_stop', 'metering_unavailable'])(
  'preserves the %s closure and its evidence',
  async (reason) => {
    const t = transport();
    const state = {
      ...enabled,
      ledger: {
        ...enabled.ledger,
        closures: [
          {
            reason,
            since: observation.observed_through,
            settings_revision: 2,
            committed_at_entry: 90,
          },
        ],
      },
    };
    t.answer(state);
    expect(await t.client.admin.readSpend()).toEqual(state);
  },
);

it.each([
  { ...observation, source: 'model_provider' },
  { ...observation, units: -1 },
  { ...observation, units: 1.5 },
  { ...observation, units: Number.MAX_SAFE_INTEGER + 1 },
  { ...observation, observed_through: null },
  { ...observation, extra: true },
])('rejects invalid observation shapes without a request', (body) => {
  const t = transport();
  expect(() => t.client.admin.recordSpendObservation(body as SpendObservationRequest)).toThrow(
    ValidationError,
  );
  expect(t.calls).toHaveLength(0);
});

it.each([0, Number.MAX_SAFE_INTEGER])('preserves the exact safe boundary %s', async (units) => {
  const t = transport();
  t.answer(enabled);
  await t.client.admin.recordSpendObservation({ ...observation, units });
  expect(JSON.parse(String(t.calls[0]!.init.body)).units).toBe(units);
});

it.each([
  { ...disabled, metering_enabled: 'false' },
  { ...enabled, committed: Number.MAX_SAFE_INTEGER + 1 },
  { ...enabled, ledger: { ...enabled.ledger, cloud: null } },
  { ...enabled, ledger: { ...enabled.ledger, closures: [{ reason: 'open' }] } },
])('rejects malformed spend responses', async (view) => {
  const t = transport();
  t.answer(view);
  await expect(t.client.admin.readSpend()).rejects.toBeInstanceOf(ProtocolError);
});

it.each([
  [403, 'forbidden'],
  [422, 'invalid_request'],
  [503, 'model_budget_writes_unavailable'],
  [503, 'platform_spend_unavailable'],
] as const)('preserves %s %s without retrying or returning an open state', async (status, code) => {
  const t = transport();
  t.answer(
    { type: `urn:cma:${code}`, title: 'Refused', status, detail: 'Synthetic refusal.', code },
    status,
  );
  const result = t.client.admin.recordSpendObservation(observation);
  await expect(result).rejects.toBeInstanceOf(ApiError);
  await expect(result).rejects.toMatchObject({ status, code });
  expect(t.calls).toHaveLength(1);
});
