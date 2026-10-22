import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { chooseService } from './test/services';

function show(path = '/') {
  window.history.replaceState(null, '', path);
  return render(
    <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
      <App />
    </AuthContextProvider>,
  );
}

beforeEach(() => {
  vi.stubGlobal(
    'fetch',
    vi.fn(async (url: string) => {
      if (url.endsWith('/auth/session'))
        return Response.json({
          authenticated: true,
          subject: 'mock-subject',
          permissions: ['cma-eg:admin'],
          expires_at: Math.floor(Date.now() / 1000) + 3600,
        });
      if (url.endsWith('/v1/channels')) return Response.json({ items: [] });
      throw new Error('Unexpected portal request: ' + url);
    }),
  );
});

afterEach(() => {
  window.history.replaceState(null, '', '/');
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

it.each([
  ['Agent Manager', '/agent-manager', 'Welcome to CMA'],
  ['Event Gateway', '/event-gateway', 'Event Gateway'],
])('expands %s from home and on direct entry', async (service, path, heading) => {
  const home = show();
  await screen.findByRole('heading', { name: 'ChronoAI Managed Agent' });
  expect(screen.queryByRole('button', { name: /(?:Show|Hide) navigation/ })).toBeNull();
  expect(screen.queryByRole('separator', { name: 'Resize navigation' })).toBeNull();
  chooseService(service as 'Agent Manager' | 'Event Gateway');
  await screen.findByRole('heading', { name: heading });
  expect(window.location.pathname).toBe(path);
  expect(screen.getByRole('button', { name: 'Hide navigation' })).toBeVisible();

  home.unmount();
  show(path);
  await screen.findByRole('heading', { name: heading });
  expect(screen.getByRole('button', { name: 'Hide navigation' })).toBeVisible();
});

it('retains manual toggles across service navigation and resets on home visits and history', async () => {
  show();
  await screen.findByRole('heading', { name: 'ChronoAI Managed Agent' });
  expect(screen.queryByRole('button', { name: /(?:Show|Hide) navigation/ })).toBeNull();
  chooseService('Agent Manager');
  await screen.findByRole('heading', { name: 'Welcome to CMA' });
  fireEvent.click(screen.getByRole('button', { name: 'Hide navigation' }));
  chooseService('Event Gateway');
  await screen.findByRole('heading', { name: 'Event Gateway' });
  expect(screen.getByRole('button', { name: 'Show navigation' })).toBeVisible();

  fireEvent.click(screen.getByRole('button', { name: 'Show navigation' }));
  chooseService('Agent Manager');
  expect(screen.getByRole('button', { name: 'Hide navigation' })).toBeVisible();
  fireEvent.click(screen.getByRole('link', { name: 'CMA home' }));
  expect(screen.queryByRole('button', { name: /(?:Show|Hide) navigation/ })).toBeNull();
  expect(screen.queryByRole('separator', { name: 'Resize navigation' })).toBeNull();

  window.history.back();
  await waitFor(() => expect(window.location.pathname).toBe('/agent-manager'));
  expect(await screen.findByRole('button', { name: 'Hide navigation' })).toBeVisible();
  window.history.forward();
  await waitFor(() => expect(window.location.pathname).toBe('/'));
  await screen.findByRole('heading', { name: 'ChronoAI Managed Agent' });
  expect(screen.queryByRole('button', { name: /(?:Show|Hide) navigation/ })).toBeNull();
});

it('keeps narrow-screen automatic collapse when entering a service from home', async () => {
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    width: 390,
    height: 844,
  } as DOMRect);
  show();
  await screen.findByRole('heading', { name: 'ChronoAI Managed Agent' });
  fireEvent.click(screen.getByRole('button', { name: 'Get Started' }));
  await screen.findByRole('heading', { name: 'Welcome to CMA' });
  expect(await screen.findByRole('button', { name: 'Show navigation' })).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Show navigation' }));
  expect(screen.getByRole('button', { name: 'Hide navigation' })).toBeVisible();
});
