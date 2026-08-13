import { describe, expect, it, vi } from 'vitest';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { IntroTerminal } from './IntroTerminal';
import { AuthContextProvider } from '../auth/context';
import { createMockAuthProvider } from '../auth/mock';
import { consumePendingTurn } from '../auth/pendingTurn';
import { setReducedMotion } from '../test/setup';
import type { AuthProvider } from '../auth/types';

/**
 * The provider resolves the identity in a microtask, so every render is
 * followed by a flush. Without it React reports an unwrapped update and the
 * assertions race the auth check.
 */
async function renderIntro(provider: AuthProvider = createMockAuthProvider()) {
  const result = render(
    <AuthContextProvider provider={provider}>
      <IntroTerminal />
    </AuthContextProvider>,
  );
  await act(async () => {});
  return result;
}

describe('IntroTerminal', () => {
  it('is interactive immediately, before the animation finishes', async () => {
    const user = userEvent.setup();
    await renderIntro();

    // The whole point of mounting a real focused input at frame zero: a
    // visitor who types at 300ms must keep every character.
    const prompt = screen.getByLabelText(/what would you like chronobot to build/i);
    await user.type(prompt, 'hello');

    expect(prompt).toHaveValue('hello');
  });

  it('exposes a complete static transcript rather than making a screen reader wait', async () => {
    await renderIntro();

    // The animated output is aria-hidden, so this is the only route to the
    // content for assistive technology and must be present from first paint.
    expect(screen.getByText(/managed agent runtime from ChronoAI/i)).toBeDefined();
    expect(screen.getByRole('heading', { level: 1 })).toBeDefined();
  });

  it('hides the animated output from assistive technology', async () => {
    const { container } = await renderIntro();
    const output = container.querySelector('.intro__out');
    expect(output?.getAttribute('aria-hidden')).toBe('true');
  });

  it('never places the input inside an aria-hidden subtree', async () => {
    await renderIntro();
    const prompt = screen.getByLabelText(/what would you like chronobot to build/i);
    expect(prompt.closest('[aria-hidden="true"]')).toBeNull();
  });

  it('captures the typed prompt and starts sign-in on Enter', async () => {
    const user = userEvent.setup();
    const provider = createMockAuthProvider();
    const signIn = vi.spyOn(provider, 'signIn');
    await renderIntro(provider);

    const prompt = screen.getByLabelText(/what would you like chronobot to build/i);
    await user.type(prompt, 'build me a web scraper{Enter}');

    await waitFor(() => {
      expect(signIn).toHaveBeenCalled();
    });
    expect(consumePendingTurn()).toBe('build me a web scraper');
  });

  it('does not sign in on a first empty Enter', async () => {
    const user = userEvent.setup();
    const provider = createMockAuthProvider();
    const signIn = vi.spyOn(provider, 'signIn');
    await renderIntro(provider);

    const prompt = screen.getByLabelText(/what would you like chronobot to build/i);
    await user.type(prompt, '{Enter}');

    expect(signIn).not.toHaveBeenCalled();
    // It must say why nothing happened rather than appearing broken.
    expect(screen.getByText(/type something first/i)).toBeDefined();
  });

  it('signs in without a prompt on a second empty Enter', async () => {
    const user = userEvent.setup();
    const provider = createMockAuthProvider();
    const signIn = vi.spyOn(provider, 'signIn');
    await renderIntro(provider);

    const prompt = screen.getByLabelText(/what would you like chronobot to build/i);
    await user.type(prompt, '{Enter}{Enter}');

    await waitFor(() => {
      expect(signIn).toHaveBeenCalled();
    });
    expect(consumePendingTurn()).toBeNull();
  });

  it('does not offer the Enter hint before anything is typed', async () => {
    await renderIntro();
    expect(screen.queryByText(/enter — sign in/i)).toBeNull();
  });

  it('offers the Enter hint once intent is established', async () => {
    const user = userEvent.setup();
    await renderIntro();

    const prompt = screen.getByLabelText(/what would you like chronobot to build/i);
    await user.type(prompt, 'fix');

    await waitFor(
      () => {
        expect(screen.getByText(/enter — sign in/i)).toBeDefined();
      },
      { timeout: 2000 },
    );
  });

  it('presents everything at once under reduced motion', async () => {
    setReducedMotion(true);
    const { container } = await renderIntro();

    // Not a degraded fallback: the same content, with the clock removed.
    expect(container.textContent).toContain("Hi, I'm ChronoBot");
    expect(container.textContent).toContain('How can I help you today?');
    // Nothing to skip, so no skip control.
    expect(screen.queryByRole('button', { name: /skip/i })).toBeNull();
    // And the affordance is visible immediately rather than gated on typing.
    expect(screen.getByText(/enter — sign in/i)).toBeDefined();
  });
});
