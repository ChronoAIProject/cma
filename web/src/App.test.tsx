import { act, fireEvent, render, screen, within } from '@testing-library/react';
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
    fireEvent.click(screen.getByRole('button', { name: 'Get Started' }));
    expect(await screen.findByRole('heading', { name: 'Welcome to CMA' })).toBeInTheDocument();
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
    const services = screen.getByRole('navigation', { name: 'CMA services' });
    expect(screen.getByRole('link', { name: 'CMA home' })).toBeInTheDocument();
    for (const name of ['Agent Manager', 'Event Gateway']) {
      expect(within(services).getByRole('link', { name })).toHaveAttribute('href');
    }
    expect(
      within(services).getByRole('button', { name: 'Image Manager (coming soon)' }),
    ).toBeDisabled();
  });
});
