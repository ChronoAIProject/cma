import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { App } from '../App';
import { Console } from '../Console';
import { CmaClientProvider } from '../api/context';
import { AuthContextProvider } from '../auth/context';
import { createMockAuthProvider } from '../auth/mock';
import type { AppRoute } from '../routing/routes';
import { agentClient } from '../test/agent';
import { observeAnimations } from '../test/animations';

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
  await screen.findByRole('treeitem', { name: /Sandbox Machine - 1/ });
  const surface = screen.getByRole('main');
  const reads = navigationReads.mock.calls.length;
  expect(motion.records).toHaveLength(0);
  rerender(draw({ kind: 'developer' }));
  expect(screen.getByRole('main')).toBe(surface);
  expect(motion.records).toHaveLength(1);
  expect(motion.records[0]?.element).toBe(surface);
  rerender(draw({ kind: 'developer' }));
  expect(motion.records).toHaveLength(1);
  rerender(draw({ kind: 'profiles' }));
  expect(motion.records).toHaveLength(2);
  expect(navigationReads).toHaveBeenCalledTimes(reads);
});

it('fades a retained Image Manager on re-entry while preserving its form and avoiding extra reads', async () => {
  window.history.replaceState({}, '', '/image-manager');
  const fetcher = vi.fn(
    async (url: string) =>
      new Response(
        JSON.stringify(
          url.endsWith('/auth/session')
            ? {
                authenticated: true,
                subject: 'owner',
                permissions: ['cma-im:admin', 'cma-eg:admin'],
                expires_at: Math.floor(Date.now() / 1000) + 3600,
              }
            : { items: [], next_cursor: null },
        ),
      ),
  );
  vi.stubGlobal('fetch', fetcher);
  render(
    <AuthContextProvider provider={createMockAuthProvider()}>
      <App />
    </AuthContextProvider>,
  );
  fireEvent.click(await screen.findByText('Create an image'));
  const field = screen.getByLabelText('Image name');
  fireEvent.change(field, { target: { value: 'Retained image draft' } });
  const surface = field.closest('.service-shell__body');
  const imageReads = () =>
    fetcher.mock.calls.filter(([url]) => url.startsWith('/api/image-manager/'));
  const reads = imageReads().length;
  expect(motion.records.filter((record) => record.element === surface)).toHaveLength(1);
  fireEvent.click(screen.getByRole('link', { name: 'Event Gateway' }));
  await screen.findByRole('button', { name: 'New channel' });
  expect(field).not.toBeVisible();
  fireEvent.click(screen.getByRole('link', { name: 'Image Manager' }));
  expect(screen.getByLabelText('Image name')).toBe(field);
  expect(field).toHaveValue('Retained image draft');
  expect(motion.records.filter((record) => record.element === surface)).toHaveLength(2);
  expect(imageReads()).toHaveLength(reads);
});
