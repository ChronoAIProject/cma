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

  it('keeps the home landing available after sign-in and enters Agent Manager without another login', async () => {
    const provider = createMockAuthProvider({ initiallyAuthenticated: true });
    const signIn = vi.spyOn(provider, 'signIn');
    render(
      <AuthContextProvider provider={provider}>
        <App />
      </AuthContextProvider>,
    );
    expect(
      await screen.findByRole('heading', { name: 'ChronoAI Managed Agent' }),
    ).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Show navigation' })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
    fireEvent.click(screen.getByRole('button', { name: 'Get Started' }));
    expect(await screen.findByRole('heading', { name: 'Welcome to CMA' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Hide navigation' })).toHaveAttribute(
      'aria-expanded',
      'true',
    );
    expect(window.location.pathname).toBe('/agent-manager');
    expect(signIn).not.toHaveBeenCalled();
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
    expect(screen.getByRole('button', { name: 'Show navigation' })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
    expect(screen.getByRole('link', { name: 'CMA home' })).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /^Service: / }));
    const services = screen.getByRole('menu', { name: 'CMA services' });
    for (const name of ['Agent Manager', 'Event Gateway']) {
      expect(within(services).getByRole('menuitemradio', { name })).toHaveAttribute('href');
    }
    expect(within(services).getByRole('menuitemradio', { name: /^Image Manager/ })).toHaveAttribute(
      'aria-disabled',
      'true',
    );
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

  it('explains an unreachable service in plain words and offers Reload', async () => {
    window.history.replaceState(null, '', '/agent-manager');
    const reload = vi.fn();
    vi.stubGlobal('location', { ...window.location, reload, pathname: '/agent-manager' });
    render(
      <AuthContextProvider
        provider={createMockAuthProvider({ failCurrent: 'identity check failed with 503' })}
      >
        <App />
      </AuthContextProvider>,
    );
    expect(
      await screen.findByRole('heading', { name: 'CMA could not reach its service' }),
    ).toBeInTheDocument();
    expect(screen.getByText('It may be starting up. Reload to try again.')).toBeInTheDocument();
    // The control plane's own sentence stays as the detail for a report.
    expect(screen.getByText('identity check failed with 503')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Reload' }));
    expect(reload).toHaveBeenCalledOnce();
    vi.unstubAllGlobals();
  });
});
