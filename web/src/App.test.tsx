import { act, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';

describe('the retained application shell', () => {
  afterEach(() => {
    window.history.replaceState(null, '', '/');
    vi.useRealTimers();
    vi.restoreAllMocks();
  });

  it('keeps the returned popup available for the next service until the parent finishes', async () => {
    vi.useFakeTimers();
    window.history.replaceState(null, '', '/connection-return?status=completed');
    const close = vi.spyOn(window, 'close').mockImplementation(() => undefined);
    render(
      <AuthContextProvider provider={createMockAuthProvider()}>
        <App />
      </AuthContextProvider>,
    );
    await act(async () => {
      await vi.advanceTimersByTimeAsync(5000);
    });
    expect(screen.getByText('Returning to CMA')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Close this window' })).toBeInTheDocument();
    expect(close).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Close this window' }));
    expect(close).toHaveBeenCalledOnce();
  });

  it('shows the authenticated empty state and account control', async () => {
    window.history.replaceState(null, '', '/agent-manager');
    render(
      <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
        <App />
      </AuthContextProvider>,
    );
    expect(await screen.findByRole('heading', { name: 'Welcome to CMA' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /account menu/i })).toBeInTheDocument();
  });

  it('replaces authenticated home with Agent Manager without painting the landing', async () => {
    const provider = createMockAuthProvider({ initiallyAuthenticated: true });
    const signIn = vi.spyOn(provider, 'signIn');
    const replace = vi.spyOn(window.history, 'replaceState');
    const push = vi.spyOn(window.history, 'pushState');
    const paintedLanding: boolean[] = [];
    const observer = new MutationObserver(() => {
      paintedLanding.push(document.querySelector('.sign-in-landing') !== null);
    });
    observer.observe(document.body, { childList: true, subtree: true });
    render(
      <AuthContextProvider provider={provider}>
        <App />
      </AuthContextProvider>,
    );
    expect(await screen.findByRole('heading', { name: 'Welcome to CMA' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Get Started' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Hide navigation' })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
    expect(window.location.pathname).toBe('/agent-manager');
    expect(replace).toHaveBeenCalledWith(
      expect.objectContaining({ cmaNavigation: { session: expect.any(String), index: 0 } }),
      '',
      '/agent-manager',
    );
    expect(push).not.toHaveBeenCalled();
    expect(signIn).not.toHaveBeenCalled();
    observer.disconnect();
    expect(paintedLanding).not.toContain(true);
  });

  it('offers sign in to an anonymous visitor', async () => {
    render(
      <AuthContextProvider provider={createMockAuthProvider()}>
        <App />
      </AuthContextProvider>,
    );
    expect(await screen.findByRole('button', { name: 'Get Started' })).toBeEnabled();
    expect(screen.getByRole('heading', { name: 'ChronoAI Managed Agent' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Sign in' })).toBeEnabled();
    expect(screen.queryByRole('button', { name: 'Show navigation' })).not.toBeInTheDocument();
    expect(screen.queryByRole('separator', { name: 'Resize navigation' })).not.toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'CMA home' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /^Service: / }));
    const services = screen.getByRole('menu', { name: 'CMA services' });
    for (const name of ['Agent Manager', 'Event Gateway']) {
      expect(within(services).getByRole('menuitemradio', { name })).toHaveAttribute('href');
    }
    expect(
      within(services).queryByRole('menuitemradio', { name: /^Image Manager/ }),
    ).not.toBeInTheDocument();
  });

  it('continues a shared Agent link after sign-in without sending its bearer fragment to OAuth', async () => {
    const secret = 'ab'.repeat(32);
    window.history.replaceState(null, '', `/agent-manager/share/share-one#${secret}`);
    const provider = createMockAuthProvider();
    const signIn = vi
      .spyOn(provider, 'signIn')
      .mockImplementation(() => new Promise<void>(() => undefined));
    render(
      <AuthContextProvider provider={provider}>
        <App />
      </AuthContextProvider>,
    );
    fireEvent.click(await screen.findByRole('button', { name: 'Sign in' }));
    await waitFor(() =>
      expect(signIn).toHaveBeenCalledWith('/agent-manager/share/share-one', undefined),
    );
    expect(window.location.hash).toBe('');
    expect(sessionStorage.getItem('cma:agent-share:share-one')).toContain(secret);
    sessionStorage.removeItem('cma:agent-share:share-one');
  });

  it('explains an identity outage and retries without reloading', async () => {
    window.history.replaceState(null, '', '/agent-manager');
    const provider = createMockAuthProvider();
    const current = vi.spyOn(provider, 'current').mockResolvedValue({
      status: 'error',
      reason: 'identity_unavailable',
      message: 'The identity service is temporarily unavailable. Retrying shortly.',
    });
    render(
      <AuthContextProvider provider={provider}>
        <App />
      </AuthContextProvider>,
    );
    expect(
      await screen.findByRole('heading', {
        name: 'The identity service is temporarily unavailable',
      }),
    ).toBeInTheDocument();
    expect(
      screen.getByText('Retrying automatically. You can also try again now.'),
    ).toBeInTheDocument();
    current.mockResolvedValue({ status: 'anonymous' });
    fireEvent.click(screen.getByRole('button', { name: 'Retry now' }));
    expect(await screen.findByRole('button', { name: 'Sign in' })).toBeInTheDocument();
    expect(current).toHaveBeenCalledTimes(2);
  });
});
