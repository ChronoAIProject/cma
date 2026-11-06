import { render, screen } from '@testing-library/react';
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
});
