import { render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';

describe('the retained application shell', () => {
  afterEach(() => {
    window.history.replaceState(null, '', '/');
    vi.restoreAllMocks();
  });

  it('renders the hosted connection return page and closes its popup', async () => {
    window.history.replaceState(null, '', '/connection-return?status=completed');
    const close = vi.spyOn(window, 'close').mockImplementation(() => undefined);
    render(
      <AuthContextProvider provider={createMockAuthProvider()}>
        <App />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('Connection complete')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Close this window' })).toBeInTheDocument();
    expect(close).not.toHaveBeenCalled();
  });

  it('shows the authenticated empty state and account control', async () => {
    render(
      <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
        <App />
      </AuthContextProvider>,
    );
    expect(await screen.findByRole('heading', { name: 'Welcome to CMA' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: /account menu/i })).toBeInTheDocument();
  });

  it('offers sign in to an anonymous visitor', async () => {
    render(
      <AuthContextProvider provider={createMockAuthProvider()}>
        <App />
      </AuthContextProvider>,
    );
    expect(await screen.findByRole('button', { name: 'Get Started' })).toBeEnabled();
    expect(screen.getByRole('heading', { name: 'ChronoAI Managed Agent' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Sign in' })).not.toBeInTheDocument();
  });
});
