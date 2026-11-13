import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { ProtocolError } from '../src/errors.js';
import { transport } from './resource-fixtures.js';

const cases = JSON.parse(
  readFileSync(
    new URL(
      '../../../api/crates/layer1-public/tests/fixtures/public-contract/admin.json',
      import.meta.url,
    ),
    'utf8',
  ),
).cases as { id: string; response: { body: Record<string, unknown> } }[];
const recorded = cases.find((entry) => entry.id === 'admin-settings-read')!.response.body;
const availabilityFields = [
  'model_budget_writes_available',
  'model_request_budget_writes_available',
  'platform_spend_writes_available',
] as const;
const availableValues = [true, false, undefined] as const;

it.each(
  availableValues.flatMap((foundation) =>
    availableValues.map((request) => ({ foundation, request })),
  ),
)('preserves independent availability $foundation/$request outside policy', async (values) => {
  const t = transport();
  const body = { ...recorded };
  for (const field of availabilityFields) delete body[field];
  const metadata = {
    ...(values.foundation === undefined
      ? {}
      : { model_budget_writes_available: values.foundation }),
    ...(values.request === undefined
      ? {}
      : { model_request_budget_writes_available: values.request }),
  };
  t.answer({ ...body, ...metadata });
  const read = await t.client.admin.readSettings();
  expect(read).toEqual({ ...body, ...metadata });
  const saved = await t.client.admin.replaceSettings({
    expected_revision: read.revision,
    settings: read.settings,
  });
  expect(saved).toEqual(read);
  expect(JSON.parse(String(t.calls.at(-1)!.init.body))).toEqual({
    expected_revision: read.revision,
    settings: read.settings,
  });
});

for (const field of availabilityFields) {
  it.each(['true', 1, null])(`rejects malformed ${field} %s on reads and saves`, async (value) => {
    const t = transport();
    t.answer(recorded);
    const read = await t.client.admin.readSettings();
    t.answer({ ...recorded, [field]: value });
    await expect(t.client.admin.readSettings()).rejects.toBeInstanceOf(ProtocolError);
    await expect(
      t.client.admin.replaceSettings({ expected_revision: read.revision, settings: read.settings }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(t.calls).toHaveLength(3);
  });
}

it.each(availableValues)(
  'preserves platform spend availability %s outside policy',
  async (value) => {
    const t = transport();
    const body = { ...recorded };
    for (const field of availabilityFields) delete body[field];
    const metadata = value === undefined ? {} : { platform_spend_writes_available: value };
    t.answer({ ...body, ...metadata });
    const read = await t.client.admin.readSettings();
    expect(read).toEqual({ ...body, ...metadata });
    const saved = await t.client.admin.replaceSettings({
      expected_revision: read.revision,
      settings: read.settings,
    });
    expect(saved).toEqual(read);
    expect(JSON.parse(String(t.calls.at(-1)!.init.body))).not.toHaveProperty(
      'platform_spend_writes_available',
    );
  },
);
