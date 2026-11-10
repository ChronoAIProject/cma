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

it.each([true, false, undefined])(
  'projects spend-write availability %s without making it policy',
  async (available) => {
    const t = transport();
    const body = { ...recorded };
    delete body.model_budget_writes_available;
    t.answer({
      ...body,
      ...(available === undefined ? {} : { model_budget_writes_available: available }),
    });
    const read = await t.client.admin.readSettings();
    expect(read.model_budget_writes_available).toBe(available);
    expect(read.settings).toEqual(recorded.settings);
    const saved = await t.client.admin.replaceSettings({
      expected_revision: read.revision,
      settings: read.settings,
    });
    expect(saved.model_budget_writes_available).toBe(available);
    expect(JSON.parse(String(t.calls.at(-1)!.init.body))).toEqual({
      expected_revision: read.revision,
      settings: read.settings,
    });
  },
);

it.each(['true', 1, null])(
  'rejects malformed availability %s instead of granting writes',
  async (available) => {
    const t = transport();
    t.answer({ ...recorded, model_budget_writes_available: available });
    await expect(t.client.admin.readSettings()).rejects.toBeInstanceOf(ProtocolError);
  },
);
