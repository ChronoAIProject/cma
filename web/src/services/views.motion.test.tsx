import { fireEvent, render, screen, within } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { App } from '../App';
import { Console } from '../Console';
import { CmaClientProvider } from '../api/context';
import { AuthContextProvider } from '../auth/context';
import { createMockAuthProvider } from '../auth/mock';
import type { AppRoute } from '../routing/routes';
import { agentClient } from '../test/agent';
import { observeAnimations } from '../test/animations';
import { chooseService } from '../test/services';

let motion: ReturnType<typeof observeAnimations>;
beforeEach(() => {
  motion = observeAnimations();
});
afterEach(() => {
  motion.restore();
  vi.unstubAllGlobals();
  window.history.replaceState({}, '', '/');
});

it('fades the existing console center between routes within Agent Manager, only when the URL changes', async () => {
  const client = agentClient();
  const navigationReads = vi.spyOn(client, 'listSandboxes');
  const draw = (route: AppRoute) => (
    <CmaClientProvider client={client}>
      <Console
        user={{ id: 'owner', handle: 'Owner', visitor: false, capabilities: ['use_own_sandboxes'] }}
        route={route}
        onNavigate={vi.fn()}
        onSignOut={vi.fn()}
      />
    </CmaClientProvider>
  );
  const { rerender } = render(draw({ kind: 'agent_manager' }));
  await screen.findByRole('treeitem', { name: /Agent one/ });
  const surface = screen.getByRole('main');
  const reads = navigationReads.mock.calls.length;
  expect(motion.records).toHaveLength(0);
  rerender(draw({ kind: 'operations' }));
  expect(screen.getByRole('main')).toBe(surface);
  expect(motion.records).toHaveLength(1);
  expect(motion.records[0]?.element).toBe(surface);
  rerender(draw({ kind: 'operations' }));
  expect(motion.records).toHaveLength(1);
  rerender(draw({ kind: 'profiles' }));
  expect(motion.records).toHaveLength(2);
  expect(navigationReads).toHaveBeenCalledTimes(reads);
});

it('fades a retained Event Gateway on re-entry while preserving its form and avoiding extra reads', async () => {
  window.history.replaceState({}, '', '/event-gateway');
  const fetcher = vi.fn(
    async (url: string) =>
      new Response(
        JSON.stringify(
          url.endsWith('/auth/session')
            ? {
                authenticated: true,
                subject: 'owner',
                permissions: ['cma-eg:admin'],
                expires_at: Math.floor(Date.now() / 1000) + 3600,
              }
            : { items: [], next_cursor: null },
        ),
      ),
  );
  vi.stubGlobal('fetch', fetcher);
  render(
    <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
      <CmaClientProvider client={agentClient()}>
        <App />
      </CmaClientProvider>
    </AuthContextProvider>,
  );
  fireEvent.click(
    await within(await screen.findByRole('region', { name: 'Channel workspace' })).findByRole(
      'button',
      { name: 'New channel' },
    ),
  );
  const field = screen.getByRole('textbox', { name: 'Channel name' });
  const draft = 'Retained channel draft';
  fireEvent.change(field, { target: { value: draft } });
  const surface = field.closest('.service-shell__body');
  const gatewayReads = () =>
    fetcher.mock.calls.filter(([url]) => url.startsWith('/api/event-gateway/'));
  const reads = gatewayReads().length;
  expect(motion.records.filter((record) => record.element === surface)).toHaveLength(1);
  chooseService('Agent Manager');
  await screen.findByRole('heading', { name: 'Welcome to CMA' });
  expect(field).not.toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: /^Service: / }));
  expect(screen.queryByRole('menuitemradio', { name: /^Image Manager/ })).not.toBeInTheDocument();
  fireEvent.keyDown(document.activeElement ?? document.body, { key: 'Escape' });
  chooseService('Event Gateway');
  expect(screen.getByRole('textbox', { name: 'Channel name' })).toBe(field);
  expect(field).toHaveValue(draft);
  expect(motion.records.filter((record) => record.element === surface)).toHaveLength(2);
  expect(gatewayReads()).toHaveLength(reads);
  expect(fetcher.mock.calls.some(([url]) => url.startsWith('/api/image-manager/'))).toBe(false);
});
