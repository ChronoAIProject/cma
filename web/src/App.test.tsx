import { describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';
import { capturePendingTurn } from './auth/pendingTurn';
import type { AuthProvider } from './auth/types';

async function renderApp(provider: AuthProvider) {
  const result = render(
    <AuthContextProvider provider={provider}>
      <App />
    </AuthContextProvider>,
  );
  // The identity check resolves in a microtask.
  await act(async () => {});
  return result;
}

/**
 * The console is code-split so the landing page does not carry xterm, which
 * means it arrives a dynamic import later than everything else.
 */
async function waitForConsole() {
  return waitFor(() => screen.getByRole('tablist', { name: /sessions/i }));
}

describe('App', () => {
  it('shows the introduction to a visitor who is not signed in', async () => {
    await renderApp(createMockAuthProvider());

    expect(screen.getByLabelText(/what would you like chronobot to build/i)).toBeInTheDocument();
  });

  it('shows the live console to a signed-in user', async () => {
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));

    expect(await waitForConsole()).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /account menu/i })).toBeInTheDocument();
  });

  it('has no navigation menu in either state', async () => {
    // The product is one page. This is the rule most likely to be eroded by a
    // later change, so it is asserted rather than merely documented.
    const { unmount } = await renderApp(createMockAuthProvider());
    expect(screen.queryByRole('navigation')).toBeNull();
    unmount();

    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    expect(screen.queryByRole('navigation')).toBeNull();
  });

  it('opens a session automatically so the console is never empty', async () => {
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    expect(screen.getAllByRole('tab').length).toBeGreaterThan(0);
  });

  it('carries the visitor prompt into the first session', async () => {
    capturePendingTurn('build me a web scraper');

    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));

    await waitFor(() => {
      expect(document.body.textContent).toContain('build me a web scraper');
    });
  });

  it('signs out back to the introduction', async () => {
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();

    await user.click(screen.getByRole('button', { name: /account menu/i }));
    await user.click(screen.getByRole('menuitem', { name: /sign out/i }));

    await waitFor(() => {
      expect(screen.getByLabelText(/what would you like chronobot to build/i)).toBeInTheDocument();
    });
  });

  it('explains an identity failure instead of rendering a blank page', async () => {
    await renderApp(createMockAuthProvider({ failCurrent: 'NyxID is unreachable.' }));

    const alert = screen.getByRole('alert');
    expect(alert).toHaveTextContent('NyxID is unreachable.');
    // Errors must say what to do about it, per Web_Style.md.
    expect(alert).toHaveTextContent(/reload/i);
  });

  it('surfaces a provider that throws rather than hanging on the check', async () => {
    const broken: AuthProvider = {
      current: () => Promise.reject(new Error('boom')),
      signIn: vi.fn(),
      signOut: vi.fn(),
    };

    await renderApp(broken);

    // The contract says implementations must not throw, but the provider is
    // injected — a misbehaving one must not leave the page permanently blank.
    expect(screen.getByRole('alert')).toHaveTextContent('boom');
  });

  it('adds a session from the + control', async () => {
    const user = userEvent.setup();
    await renderApp(createMockAuthProvider({ initiallyAuthenticated: true }));
    await waitForConsole();
    const before = screen.getAllByRole('tab').length;

    await user.click(screen.getByRole('button', { name: /new session/i }));

    expect(screen.getAllByRole('tab')).toHaveLength(before + 1);
  });
});
