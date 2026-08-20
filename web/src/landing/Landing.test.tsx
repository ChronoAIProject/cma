import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { Landing } from './Landing';
import { AuthContextProvider } from '../auth/context';
import { createMockAuthProvider } from '../auth/mock';
import { clearPendingTurn, consumePendingTurn } from '../auth/pendingTurn';
import type { AuthProvider } from '../auth/types';

/**
 * The signed-out surface — §20.
 *
 * What is worth asserting is not the mark: an <img> either has a src or the
 * build failed, and nothing here can see whether it animated. It is the two
 * behaviours the page exists for — it reaches sign-in, and it does not drop
 * what a visitor typed on the way there.
 *
 * The provider resolves identity in a microtask, so every render is flushed.
 * Without it React reports an unwrapped update and the assertions race it.
 */
async function renderLanding(provider: AuthProvider) {
  const result = render(
    <AuthContextProvider provider={provider}>
      <Landing />
    </AuthContextProvider>,
  );
  await act(async () => {});
  return result;
}

/** A provider whose signIn is observable, since the redirect itself is not. */
function providerSpy() {
  const provider = createMockAuthProvider();
  const signIn = vi.fn(async () => {
    await Promise.resolve();
  });
  return { provider: { ...provider, signIn }, signIn };
}

const PROMPT = /what would you like cma to build/i;

describe('Landing', () => {
  beforeEach(() => {
    clearPendingTurn();
  });

  it('focuses the prompt on mount so a visitor can type immediately', async () => {
    const { provider } = providerSpy();
    await renderLanding(provider);
    expect(screen.getByLabelText(PROMPT)).toHaveFocus();
  });

  it('holds the typed prompt across sign-in', async () => {
    const { provider, signIn } = providerSpy();
    const user = userEvent.setup();
    await renderLanding(provider);

    await user.type(screen.getByLabelText(PROMPT), 'build a scraper');
    await user.click(screen.getByRole('button', { name: /sign in and run it/i }));

    await waitFor(() => {
      expect(signIn).toHaveBeenCalledTimes(1);
    });
    expect(consumePendingTurn()).toBe('build a scraper');
  });

  it('signs in without a prompt, holding nothing', async () => {
    const { provider, signIn } = providerSpy();
    const user = userEvent.setup();
    await renderLanding(provider);

    await user.click(screen.getByRole('button', { name: /^sign in$/i }));

    await waitFor(() => {
      expect(signIn).toHaveBeenCalledTimes(1);
    });
    // An empty field must not capture: a stored empty turn would make the
    // first session open by sending nothing, which reads as a stuck agent.
    expect(consumePendingTurn()).toBeNull();
  });

  it('submits on Enter from the prompt', async () => {
    const { provider, signIn } = providerSpy();
    const user = userEvent.setup();
    await renderLanding(provider);

    await user.type(screen.getByLabelText(PROMPT), 'ship it{Enter}');

    await waitFor(() => {
      expect(signIn).toHaveBeenCalledTimes(1);
    });
    expect(consumePendingTurn()).toBe('ship it');
  });

  it('refuses a second submission while the redirect is in flight', async () => {
    const { provider, signIn } = providerSpy();
    const user = userEvent.setup();
    await renderLanding(provider);

    const button = screen.getByRole('button', { name: /^sign in$/i });
    await user.click(button);
    await waitFor(() => {
      expect(button).toBeDisabled();
    });
    await user.click(button);

    expect(signIn).toHaveBeenCalledTimes(1);
  });
});
