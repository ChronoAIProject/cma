// @vitest-environment node
import { expect, it } from 'vitest';
import { ProtocolError } from '../errors.js';
import { launchFixture } from './launch-fixtures.js';
import { transport } from './resource-fixtures.js';

it('reads the immutable first-message association and retains legacy omission without a POST', async () => {
  const test = transport();
  const association = {
    agent_id: 'agt_original',
    command_id: 'cmd_original',
    response_id: 'rsp_original',
  };
  const current = { ...launchFixture, first_message_command: association };
  test.answer(current);
  expect((await test.client.launches.read(current.id)).first_message_command).toEqual(association);
  test.answer({ items: [current, launchFixture] });
  const page = await test.client.launches.list();
  expect(page.items[0]?.first_message_command).toEqual(association);
  expect(page.items[1]?.first_message_command).toBeUndefined();
  test.answer(launchFixture);
  expect((await test.client.launches.read(current.id)).first_message_command).toBeUndefined();
  expect(test.calls.every(({ init }) => !init.method || init.method === 'GET')).toBe(true);
});

it('refuses malformed association shapes without recovery submission', async () => {
  const test = transport();
  for (const association of [
    { agent_id: 'agt_original', command_id: 'cmd_original' },
    { agent_id: 4, command_id: 'cmd_original', response_id: 'rsp_original' },
  ]) {
    test.answer({ ...launchFixture, first_message_command: association });
    await expect(test.client.launches.read(launchFixture.id)).rejects.toBeInstanceOf(ProtocolError);
  }
  expect(test.calls).toHaveLength(2);
});
