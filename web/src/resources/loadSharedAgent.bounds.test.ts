import { afterEach, expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { agentClient } from '../test/agent';
import { historyPage, statePage } from '../agent/historyWindow.fixture';
import {
  authorizeSharedHistory,
  loadSharedAgent,
  loadSharedAgentAccess,
  loadSharedAgentTranscript,
  SharedAgentAccessChangedError,
} from './loadSharedAgent';

afterEach(() => vi.useRealTimers());
function fixture() {
  const access = {
    policy: 1,
    capabilities: ['view_history'],
    missing: false,
    afterHistory: () => {},
  };
  const fetch = vi.fn(async (input: RequestInfo | URL) => {
    const path = new URL(String(input)).pathname;
    if (path.includes('/resource-access/'))
      return Response.json({
        capabilities: access.capabilities,
        revalidate_at: new Date(Date.now() + 50_000).toISOString(),
        policy_revision: access.policy,
        shared_by: { subject: 'fixture-owner', display_name: 'Fixture', avatar_url: null },
      });
    if (access.missing) return Response.json({ code: 'resource_not_found' }, { status: 404 });
    if (path.endsWith('/state')) return Response.json(statePage());
    if (path.endsWith('/history')) {
      access.afterHistory();
      return Response.json(historyPage());
    }
    throw new Error('Unexpected shared read');
  });
  const sdk = createClient({ baseUrl: 'https://fixture.invalid', fetch });
  const transcript = vi.fn(async () => ({ agentId: 'agt_1', throughSeq: 0, turns: [] }));
  const ancestry = vi.fn();
  const web = agentClient({
    agentHistory: sdk.agentEventsV2,
    readAgentTranscript: transcript,
    readAgent: ancestry,
    readWorkspace: ancestry,
    readSandbox: ancestry,
  });
  return { access, fetch, sdk, web, transcript, ancestry, signal: new AbortController().signal };
}

it('loads one shared window and current controls using only Agent-scoped authority', async () => {
  const h = fixture();
  const reading = await loadSharedAgent(h.web, h.sdk, 'agt_1', h.signal);
  expect(reading.value.conversation.messages).toHaveLength(2);
  expect(reading.value.conversation.approvals).toHaveLength(1);
  expect(h.ancestry).not.toHaveBeenCalled();
  expect(h.transcript).not.toHaveBeenCalled();
  expect(h.fetch).toHaveBeenCalledTimes(4);
});

it('rejects a shared window when policy changes while it is being read', async () => {
  const h = fixture();
  h.access.afterHistory = () => {
    h.access.policy++;
  };
  await expect(loadSharedAgent(h.web, h.sdk, 'agt_1', h.signal)).rejects.toBeInstanceOf(
    SharedAgentAccessChangedError,
  );
});

it('revalidates shared authority before falling back to a predecessor transcript', async () => {
  const h = fixture();
  const authorization = await loadSharedAgentAccess(h.sdk, 'agt_1', h.signal);
  h.access.missing = true;
  h.access.capabilities = [];
  await expect(
    loadSharedAgentTranscript(h.web, h.sdk, authorization.value, h.signal),
  ).rejects.toBeInstanceOf(SharedAgentAccessChangedError);
  expect(h.transcript).not.toHaveBeenCalled();
});

it('expires shared navigation without granting a new deadline from later access reads', async () => {
  vi.useFakeTimers();
  const h = fixture();
  const authorization = await loadSharedAgentAccess(h.sdk, 'agt_1', h.signal);
  const changed = vi.fn();
  vi.setSystemTime(Date.now() + 51_000);
  await expect(
    authorizeSharedHistory(h.sdk, authorization.value, h.signal, changed),
  ).rejects.toBeInstanceOf(SharedAgentAccessChangedError);
  expect(changed).toHaveBeenCalledTimes(1);
  expect(h.fetch).toHaveBeenCalledTimes(1);
});
