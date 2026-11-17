// @vitest-environment node
import { expect, it } from 'vitest';
import { ApiError, ProtocolError } from '../index.js';
import { transport } from './resource-fixtures.js';

const defaults = {
  sandbox_profile: { id: 'sbp_default', revision: 2 },
  workspace_profile: { id: 'wkp_default', revision: 3 },
  agent_profile: { id: 'agp_default', revision: 4 },
};

it('reads current owner-visible defaults and omits private extensions', async () => {
  const t = transport();
  t.answer({ ...defaults, private_settings: 'must-not-cross' });
  await expect(t.client.readDefaults()).resolves.toEqual(defaults);
  expect(t.calls[0]?.url).toBe('https://cma.example.test/api/v1/defaults');
  expect(t.calls[0]?.init.credentials).toBe('include');
});

it.each([undefined, null, 0, -1, 1.5, Number.MAX_SAFE_INTEGER + 1])(
  'rejects invalid pinned revision %s',
  async (revision) => {
    const t = transport();
    t.answer({ ...defaults, sandbox_profile: { id: 'sbp_default', revision } });
    await expect(t.client.readDefaults()).rejects.toBeInstanceOf(ProtocolError);
  },
);

it('enforces success status and preserves owner-authorization failures', async () => {
  const t = transport();
  t.answer(defaults, 201);
  await expect(t.client.readDefaults()).rejects.toBeInstanceOf(ProtocolError);
  t.answer({ code: 'forbidden', detail: 'An owner capability is required.' }, 403);
  await expect(t.client.readDefaults()).rejects.toBeInstanceOf(ApiError);
});
