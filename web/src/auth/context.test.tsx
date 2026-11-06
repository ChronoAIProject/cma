import { act, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { AuthContextProvider, useAuth } from './context';
import type { AuthProvider } from './types';

function Status() {
  return <p>{useAuth().state.status}</p>;
}

function provider(signInSilently?: () => boolean): AuthProvider {
  return {
    current: async () => ({ status: 'anonymous' }),
    signIn: async () => {},
    signOut: async () => {},
    signInSilently,
  };
}

describe('the identity check', () => {
  it('stays checking while a silent sign-in leaves the page', async () => {
    const signInSilently = vi.fn(() => true);
    render(
      <AuthContextProvider provider={provider(signInSilently)}>
        <Status />
      </AuthContextProvider>,
    );
    await vi.waitFor(() => expect(signInSilently).toHaveBeenCalledTimes(1));
    expect(screen.getByText('checking')).toBeInTheDocument();
  });

  it('reports anonymous when no silent sign-in was started', async () => {
    render(
      <AuthContextProvider provider={provider(() => false)}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
  });

  it('reports anonymous for a provider that cannot sign in silently', async () => {
    render(
      <AuthContextProvider provider={provider()}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
  });

  it('shows the landing when a silent sign-in never leaves the page', async () => {
    vi.useFakeTimers();
    try {
      render(
        <AuthContextProvider provider={provider(() => true)}>
          <Status />
        </AuthContextProvider>,
      );
      await act(async () => {});
      expect(screen.getByText('checking')).toBeInTheDocument();
      await act(async () => {
        vi.advanceTimersByTime(10000);
      });
      expect(screen.getByText('anonymous')).toBeInTheDocument();
    } finally {
      vi.useRealTimers();
    }
  });

  it('ignores focus while a silent sign-in is leaving the page', async () => {
    const current = vi.fn(async () => ({ status: 'anonymous' }) as const);
    render(
      <AuthContextProvider provider={{ ...provider(() => true), current }}>
        <Status />
      </AuthContextProvider>,
    );
    await vi.waitFor(() => expect(current).toHaveBeenCalledTimes(1));
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
    });
    expect(current).toHaveBeenCalledTimes(1);
    expect(screen.getByText('checking')).toBeInTheDocument();
  });

  it('never leaves the page from a later refresh', async () => {
    const signInSilently = vi.fn(() => false);
    render(
      <AuthContextProvider provider={provider(signInSilently)}>
        <Status />
      </AuthContextProvider>,
    );
    expect(await screen.findByText('anonymous')).toBeInTheDocument();
    signInSilently.mockReturnValue(true);
    await act(async () => {
      window.dispatchEvent(new Event('focus'));
    });
    expect(screen.getByText('anonymous')).toBeInTheDocument();
    expect(signInSilently).toHaveBeenCalledTimes(1);
  });
});
