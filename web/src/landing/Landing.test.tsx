import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
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
const css = () => readFileSync(resolve(process.cwd(), 'src/landing/Landing.css'), 'utf8');

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

describe('the signed-out appearance', () => {
  it('draws the two signature grounds entirely from appearance tokens', () => {
    const sheet = css();
    const light = /\.landing\s*\{([^}]*)\}/.exec(sheet)?.[1] ?? '';
    expect(light).toContain('background-color: var(--dsw-alias-bg-base)');
    expect(light).toContain('repeating-linear-gradient');
    expect(light).toContain(
      'color-mix(in srgb, var(--dsw-alias-state-business-primary) 7%, transparent) 27px 28px',
    );
    expect(light).toContain(
      'color-mix(in srgb, var(--dsw-alias-state-error-primary) 18%, transparent) 47px 48px',
    );

    const dark = /body\[data-ds-dark-theme\] \.landing\s*\{([^}]*)\}/.exec(sheet)?.[1] ?? '';
    expect(dark).toContain('radial-gradient');
    expect(dark).toContain(
      'color-mix(in srgb, var(--dsw-alias-label-primary) 3%, transparent) 1px',
    );
    expect(dark).toContain('background-size: 18px 18px');
  });

  it('uses themed display weight and depth without changing the mark geometry', () => {
    const sheet = css();
    const mark = /\.landing__mark\s*\{([^}]*)\}/.exec(sheet)?.[1] ?? '';
    expect(mark).toContain('filter: drop-shadow(var(--dsw-shadow-lv1-blur))');
    expect(sheet).toMatch(
      /body\[data-ds-dark-theme\] \.landing__mark\s*\{[^}]*filter: drop-shadow\(var\(--dsw-glow-accent\)\)/,
    );

    const word = /\.landing__word\s*\{([^}]*)\}/.exec(sheet)?.[1] ?? '';
    expect(word).toContain('font-family: var(--dsw-font-family)');
    expect(word).toContain('font-weight: 700');
    expect(word).toContain('font-variation-settings: var(--ds-font-variation-display)');
    expect(sheet).toMatch(/body\[data-ds-dark-theme\] \.landing__word\s*\{[^}]*font-weight: 900/);
  });

  it('gives the prompt the shared field scale and soft accent focus', () => {
    const sheet = css();
    expect(sheet).toMatch(
      /\.landing__prompt\s*\{[^}]*height: 44px[^}]*background: var\(--dsw-alias-bg-layer-1\)[^}]*border-radius: 999px/,
    );
    expect(sheet).toMatch(
      /\.landing__prompt:focus-visible\s*\{[^}]*border-color: var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--dsw-ring-accent-soft\)/,
    );
  });
});

describe('the mark bobs, and only once it is drawn', () => {
  /*
   * Asserted against the sheet rather than a rendered node: jsdom runs no
   * animations and resolves no custom property, so a computed style here would
   * report neither the timing nor the curve.
   *
   * `docs/Web_Style.md` §14a permits this and nothing else to loop, under
   * stated terms. Each of those terms is one assertion below, so relaxing one
   * is a decision somebody makes rather than a drift nobody notices.
   */
  it('waits for the drawing to finish before it starts', () => {
    // The mark's own file fades the bridge in last, at 1.78s over 0.3s. A bob
    // starting before 2.08s drags a half-drawn mark around.
    const rule = /\.landing__mark\s*\{([^}]*)\}/.exec(css())?.[1] ?? '';
    const delay = /animation:[^;]*?(\d+(?:\.\d+)?)s\s+infinite/.exec(rule);
    expect(rule).toMatch(/animation:\s*landing-bob/);
    expect(Number(delay?.[1] ?? 0)).toBeGreaterThanOrEqual(2.08);
  });

  it('moves up and down and does nothing else', () => {
    // §14a: no rotation, no scale, no travelling shadow. A mark that tilts or
    // swells reads as a toy.
    const frames = /@keyframes landing-bob\s*\{([\s\S]*?)\n\}/.exec(css())?.[1] ?? '';
    expect(frames).toMatch(/translateY/);
    expect(frames).not.toMatch(/rotate|scale|skew|box-shadow|opacity/);
  });

  it('runs on the crop, never on the artwork inside it', () => {
    /*
     * The wrapper clips the square file to the mark's proportions. Animating
     * the `svg` pushes the arc's apex into that clipped edge and shears the
     * top off at the peak of every bounce; animating the wrapper takes the
     * window and its contents together.
     */
    const svgRule = /\.landing__mark svg\s*\{([^}]*)\}/.exec(css())?.[1] ?? '';
    expect(svgRule).not.toMatch(/animation/);
  });

  it('uses the one curve, and stops at rest for reduced motion', () => {
    const sheet = css();
    expect(sheet).toMatch(/animation:\s*landing-bob[^;]*var\(--ds-ease-in-out\)/);
    expect(sheet).not.toMatch(/cubic-bezier/);
    const reduced =
      /@media \(prefers-reduced-motion: reduce\)\s*\{([\s\S]*?)\n\}/.exec(sheet)?.[1] ?? '';
    expect(reduced).toMatch(/\.landing__mark\s*\{[^}]*animation:\s*none/);
    // At rest, not frozen mid-travel.
    expect(reduced).toMatch(/\.landing__mark\s*\{[^}]*transform:\s*none/);
  });
});
