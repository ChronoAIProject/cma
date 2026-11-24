import { useState } from 'react';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { connectionClient } from '../connections/client';
import { accessRequests } from '../agent/accessRequests';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { referenceClient } from '../test/references';
import { sandboxFixture } from '../test/hierarchy';
import { SandboxSections } from './SandboxSections';
import type { SandboxTab } from './sandboxTabRoute';

afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
});

it('enables a changed service scope after opening the Access tab', async () => {
  vi.spyOn(connectionClient, 'check').mockResolvedValue({
    owner: 'owner',
    ready: true,
    checked_at: '2026-11-24T00:00:00Z',
    services: [],
  });
  const replace = vi.spyOn(accessRequests, 'replace').mockResolvedValue({
    id: 'access-1',
    agent_id: AGENT_DETAIL.id,
    status: 'succeeded',
    services: ['chrono-llm-public', 'ornn-api'],
    reason: 'Owner revised scope.',
  });
  const client = agentClient({
    readAgent: async () => ({
      ...AGENT_DETAIL,
      reach: ['chrono-llm-public', 'ornn-api', 'storage'],
    }),
  });
  function Page() {
    const [selected, onSelect] = useState<SandboxTab>('overview');
    return (
      <CmaClientProvider client={client} profiles={referenceClient()}>
        <SandboxSections
          client={client}
          sandbox={sandboxFixture()}
          owner="owner"
          observedAt="2026-11-24T00:00:00Z"
          permissions={{
            createWorkspace: false,
            createAgent: false,
            pauseSandbox: false,
            deleteSandbox: false,
            removeWorkspace: false,
            deleteAgent: false,
            stopAgent: false,
            exportHistory: false,
          }}
          busy={false}
          stale={false}
          readEnabled
          selected={selected}
          onSelect={onSelect}
          onAction={vi.fn()}
          onNavigate={vi.fn()}
        />
      </CmaClientProvider>
    );
  }
  const user = userEvent.setup();
  render(<Page />);
  await user.click(screen.getByRole('tab', { name: 'Access' }));
  await user.click(await screen.findByRole('checkbox', { name: 'Chrono Storage' }));
  await waitFor(() => expect(screen.getByRole('button', { name: 'Save scope' })).toBeEnabled());
  await user.click(screen.getByRole('button', { name: 'Save scope' }));
  await screen.findByText('Agent key updated. Your Agent has been notified.');
  expect(replace).toHaveBeenCalledOnce();
});
