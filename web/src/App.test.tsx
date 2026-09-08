import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { App } from './App';
import { AuthContextProvider } from './auth/context';
import { createMockAuthProvider } from './auth/mock';

describe('the retained application shell', () => {
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
