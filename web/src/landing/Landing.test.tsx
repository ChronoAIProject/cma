import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { Landing } from './Landing';
import { AuthContextProvider } from '../auth/context';
import { createMockAuthProvider } from '../auth/mock';
import { clearPendingTurn, consumePendingTurn } from '../auth/pendingTurn';
import type { AuthProvider } from '../auth/types';

const characterScene = vi.hoisted(() => ({ mount: vi.fn(), dispose: vi.fn() }));

vi.mock('./characterScene', () => ({
  mountCharacter: (
    host: HTMLElement,
    options: { readonly onReady?: () => void },
  ): { readonly dispose: () => void } => {
    characterScene.mount();
    if (host instanceof HTMLButtonElement) {
      host.disabled = false;
    }
    const canvas = document.createElement('canvas');
    canvas.className = 'landing__character-canvas';
    host.appendChild(canvas);
    options.onReady?.();
    return { dispose: characterScene.dispose };
  },
}));

/**
 * The signed-out surface — §20.
 *
 * The WebGL pixels are measured in a real browser. Here the scene is replaced
 * by its lifecycle boundary so the two page behaviours stay deterministic: it
 * reaches sign-in, and it does not drop what a visitor typed on the way there.
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
    vi.stubGlobal('WebGL2RenderingContext', class WebGL2RenderingContext {});
    clearPendingTurn();
    characterScene.mount.mockClear();
    characterScene.dispose.mockClear();
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('focuses the prompt on mount so a visitor can type immediately', async () => {
    const { provider } = providerSpy();
    await renderLanding(provider);
    expect(screen.getByLabelText(PROMPT)).toHaveFocus();
  });

  it('reserves the character stage while its deferred scene mounts', async () => {
    const { provider } = providerSpy();
    const result = await renderLanding(provider);
    const character = screen.getByRole('button', { name: 'Spin CMA character' });

    expect(character).toHaveAttribute('data-ready');
    expect(character).toHaveAttribute('title', 'Spin CMA character');
    expect(character).toBeEnabled();
    expect(character.querySelector('.landing__character-fallback svg')).not.toBeNull();
    expect(character.querySelector('canvas')).not.toBeNull();
    expect(characterScene.mount).toHaveBeenCalledOnce();

    result.unmount();
    expect(characterScene.dispose).toHaveBeenCalledOnce();
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

  it('reserves a square character stage and themes its static fallback', () => {
    const sheet = css();
    const character = /\.landing__character\s*\{([^}]*)\}/.exec(sheet)?.[1] ?? '';
    expect(character).toContain('width: clamp(220px, 50vmin, 520px)');
    expect(character).toContain('aspect-ratio: 1');
    expect(sheet).toMatch(
      /\.landing__character-fallback\s*\{[^}]*box-sizing: border-box[^}]*color: var\(--dsw-alias-brand-primary\)[^}]*filter: drop-shadow\(var\(--dsw-shadow-lv1-blur\)\)/,
    );
    expect(sheet).toMatch(
      /body\[data-ds-dark-theme\] \.landing__character-fallback\s*\{[^}]*filter: drop-shadow\(var\(--dsw-glow-accent\)\)/,
    );
    expect(sheet).toMatch(
      /\.landing__character\[data-ready\] \.landing__character-fallback\s*\{[^}]*visibility: hidden/,
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
