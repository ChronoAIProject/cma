// @vitest-environment node
import { expect, it } from 'vitest';
import { ProtocolError } from '../errors.js';
import { bindingHoldOverview } from '../admin/binding-holds.js';
import { heldBindings } from '../../operator/bindingHolds.test-support';
import { transport } from './resource-fixtures.js';

it('reads the authorized observation without a write or cached response', async () => {
  const t = transport();
  t.answer(heldBindings());
  const controller = new AbortController();
  expect(await t.client.admin.bindingHolds({ signal: controller.signal })).toEqual(heldBindings());
  expect(t.calls).toHaveLength(1);
  expect(new URL(t.calls[0]!.url).pathname).toBe('/api/v1/admin/runtime-binding-holds');
  expect(t.calls[0]!.init.method).toBe('GET');
  expect(t.calls[0]!.init.cache).toBe('no-store');
  expect(t.calls[0]!.init.body).toBeUndefined();
});

it.each([
  { observed_sandboxes: 2 },
  { registered_reporters: 4 },
  { unavailable_reporters: -1 },
  { registered_reporters: 17 },
  { observed_at: 'not a timestamp' },
  { reports: Array.from({ length: 17 }, () => heldBindings().reports[0]) },
  { reports: [heldBindings().reports[0], heldBindings().reports[0]] },
])('refuses inconsistent, malformed or excessive summaries %j', (patch) => {
  expect(() => bindingHoldOverview({ ...heldBindings(), ...patch })).toThrow(ProtocolError);
});

it.each([
  { attempts: 0 },
  { attempts: 4_294_967_296 },
  { phase: 'completed' },
  { retry_not_before: null },
  { reason: 'raw provider error' },
  { adapter_error_class: 'raw provider error' },
  { first_held_at: 'tomorrow' },
  { owner: '' },
  { owner_name: 'Alex\nChen' },
  { owner_name: `Alex${String.fromCharCode(127)}Chen` },
  { sandbox_id: 's'.repeat(513) },
])('refuses invalid held rows %j', (patch) => {
  const value = heldBindings();
  const first = value.reports[0]!;
  expect(() =>
    bindingHoldOverview({
      ...value,
      reports: [
        {
          ...first,
          sample: {
            ...first.sample,
            holds: [{ ...first.sample.holds[0], ...patch }],
          },
        },
        value.reports[1],
      ],
    }),
  ).toThrow(ProtocolError);
});

it('refuses duplicate or excessive rows and concealed truncation', () => {
  const value = heldBindings();
  const first = value.reports[0]!;
  for (const sample of [
    { ...first.sample, holds: [first.sample.holds[0], first.sample.holds[0]] },
    { ...first.sample, holds: Array.from({ length: 65 }, () => first.sample.holds[0]) },
    { ...first.sample, truncated: true },
  ]) {
    expect(() =>
      bindingHoldOverview({ ...value, reports: [{ ...first, sample }, value.reports[1]] }),
    ).toThrow(ProtocolError);
  }
});

it('preserves a zero observation with missing reporters and discards undisplayed extra fields', () => {
  const value = {
    observed_at: heldBindings().observed_at,
    observed_sandboxes: 0,
    registered_reporters: 1,
    unavailable_reporters: 1,
    truncated: true,
    reports: [],
  };
  expect(bindingHoldOverview({ ...value, raw_error: 'private-debug-message' })).toEqual(value);
});
