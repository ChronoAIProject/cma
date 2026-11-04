import { expect, it, vi } from 'vitest';
import { createClient, ProtocolError } from '../src/index.js';

const policy = {
  object: 'cma.ui.task_experience',
  schema_version: 1,
  account_id: 'owner-a',
  capabilities: {
    task_home_admission: true,
    suggested_followup_admission: false,
    desktop_enable: false,
    browser_takeover: false,
    replay_presentation: true,
  },
};

it('reads authenticated task eligibility on every call and rejects an incompatible policy', async () => {
  const fetch = vi
    .fn()
    .mockResolvedValueOnce(Response.json(policy))
    .mockResolvedValueOnce(Response.json({ ...policy, schema_version: 2 }));
  const client = createClient({
    baseUrl: 'https://cma.example',
    credential: () => 'fixture-credential',
    fetch,
  });
  expect(await client.taskExperience.read()).toEqual(policy);
  expect(String(fetch.mock.calls[0]?.[0])).toBe('https://cma.example/api/v2/ui/task-experience');
  expect(new Headers(fetch.mock.calls[0]?.[1]?.headers).get('Authorization')).toBe(
    'Bearer fixture-credential',
  );
  await expect(client.taskExperience.read()).rejects.toThrow(ProtocolError);
  expect(fetch).toHaveBeenCalledTimes(2);
});
