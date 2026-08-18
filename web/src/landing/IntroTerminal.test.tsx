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
    expect(screen.queryByText(/enter to continue/i)).toBeNull();
  });

  it('offers the Enter hint once intent is established', async () => {
    const user = userEvent.setup();
    await renderIntro();

    const prompt = screen.getByLabelText(/what would you like chronobot to build/i);
    await user.type(prompt, 'fix');

    await waitFor(
      () => {
        expect(screen.getByText(/enter to continue/i)).toBeDefined();
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
    expect(screen.getByText(/enter to continue/i)).toBeDefined();
  });
});

describe('§15 the signed-out surface', () => {
  it('focuses the prompt on first paint', async () => {
    // §10 and §15: focus on load rests on the prompt, so a keyboard visitor
    // can type immediately without hunting. Time-to-interactive is 0ms, not
    // 3.2 seconds — the animation is decoration over a usable field.
    await renderIntro();
    expect(document.activeElement).toBe(screen.getByRole('textbox'));
  });

  it('keeps every character a visitor types at 300ms', async () => {
    const user = userEvent.setup();
    await renderIntro();
    await user.keyboard('open a pull request');
    expect(screen.getByRole('textbox')).toHaveValue('open a pull request');
  });

  it('completes the animation on a click, not only on Escape', async () => {
    // §15: any keypress or click completes it. A click that only moved focus
    // left the gesture a visitor reaches for when they have decided the
    // introduction is an obstacle doing nothing.
    const user = userEvent.setup();
    const { container } = await renderIntro();
    const frame = container.querySelector('.intro__frame') as HTMLElement;

    await user.click(frame);

    expect(container.querySelector('.intro__row')).toHaveAttribute('data-live', 'true');
  });

  it('carries the accessible transcript before any animation starts', async () => {
    // §10: the introduction is never the only route to its information, and a
    // screen reader user never waits for an animation.
    const { container } = await renderIntro();
    const transcript = container.querySelector('.sr-only');
    expect(transcript?.textContent).toContain('chrono-bot, a managed agent runtime from ChronoAI.');
    expect(transcript?.textContent).toContain('How can I help you today?');
  });

  it('states what the product is, in words a visitor can read', async () => {
    // §15 opens with "a statement, and a terminal that already works". It was
    // a screen-reader-only heading, which is a statement nobody sighted saw.
    await renderIntro();
    expect(screen.getByRole('heading', { level: 1 })).toBeVisible();
  });

  it('does not nag an empty prompt', async () => {
    // §15: the sign-in affordance appears only once the visitor has typed a
    // character.
    const user = userEvent.setup();
    await renderIntro();
    expect(screen.queryByRole('button', { name: /sign in and run it/i })).toBeNull();

    await user.keyboard('a');
    expect(screen.getByRole('button', { name: /sign in and run it/i })).toBeInTheDocument();
  });
});
