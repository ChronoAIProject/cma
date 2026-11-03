import { render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { createClient } from '@chronoai/cma-sdk';
import { ScopedNavigation } from './ScopedNavigation';

it.each(['shared', 'organization:research'])(
  'queries scoped Agents directly with status and resets pagination on a changed filter (%s)',
  async (scope) => {
    const requests: URL[] = [];
    const client = createClient({
      baseUrl: 'https://fixture.invalid',
      fetch: async (input) => {
        const url = new URL(String(input));
        requests.push(url);
        expect(url.pathname).toBe('/api/v1/agents/provenance');
        expect(url.searchParams.get('shared')).toBe(scope === 'shared' ? 'true' : 'false');
        expect(url.searchParams.get('organization')).toBe(scope === 'shared' ? null : 'research');
        return Response.json({ items: [] });
      },
    });
    const props = {
      client,
      scope,
      view: 'agent' as const,
      personal: <p>Personal tree</p>,
      choices: { options: [], selected: undefined, unavailable: false },
      onNavigate: vi.fn(),
    };
    const view = render(<ScopedNavigation {...props} statusFilter="working" />);
    await screen.findByText('No Agents match this scope and status on this page.');
    expect(requests[0]?.searchParams.get('task_status')).toBe('working');
    view.rerender(<ScopedNavigation {...props} statusFilter="not_started" />);
    await waitFor(() => expect(requests).toHaveLength(2));
    expect(requests[1]?.searchParams.get('task_status')).toBe('not_started');
    expect(requests[1]?.searchParams.has('cursor')).toBe(false);
    expect(screen.queryByText('Personal tree')).not.toBeInTheDocument();
  },
);
