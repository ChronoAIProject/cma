import { afterEach, expect, it, vi } from 'vitest';
import { agentClient } from '../test/agent';
import { createClient } from '@chronoai/cma-sdk';
import { createAgentTransport } from './transport/client';
import { agentTransportJournal, clearAgentTransport } from './transport/journal';
import { readCooldown, rememberReadCooldown } from './readCooldown';

afterEach(() => {
  sessionStorage.clear();
  vi.useRealTimers();
  vi.restoreAllMocks();
});
it('isolates client, account, Agent and sign-in epochs without retaining authority', () => {
  agentTransportJournal(sessionStorage, 'alice', 'a');
  const client = agentClient({
    agentTransport: createAgentTransport(createClient({ baseUrl: 'https://fixture.invalid' }), {
      storage: sessionStorage,
      currentUser: async () => 'alice',
    }),
  });
  const at = Date.now() + 90_000;
  rememberReadCooldown(client, 'alice', 'a', at);
  expect(readCooldown(client, 'alice', 'a')).toBe(at);
  expect(readCooldown(client, 'alice', 'b')).toBe(0);
  expect(readCooldown(agentClient(), 'alice', 'a')).toBe(0);
  clearAgentTransport(sessionStorage);
  agentTransportJournal(sessionStorage, 'alice', 'a');
  expect(readCooldown(client, 'alice', 'a')).toBe(0);
  rememberReadCooldown(client, 'alice', 'a', at);
  expect(readCooldown(client, 'bob', 'a')).toBe(0);
  expect(readCooldown(client, 'alice', 'a')).toBe(0);
});
it('bounds stored deadlines without evicting live retry floors', () => {
  vi.useFakeTimers();
  const client = agentClient();
  const at = Date.now() + 90_000;
  for (let i = 0; i < 200; i++) rememberReadCooldown(client, 'alice', String(i), at + i);
  expect(readCooldown(client, 'alice', '0')).toBe(at);
  expect(readCooldown(client, 'alice', '199')).toBe(at + 199);
  vi.advanceTimersByTime(91_000);
  expect(readCooldown(client, 'alice', '199')).toBe(0);
});
