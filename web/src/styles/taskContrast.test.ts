import { expect, it } from 'vitest';
import { resolveToken, themes } from '../test/palette';

function luminance(hex: string) {
  const channels = [1, 3, 5].map(
    (offset) => Number.parseInt(hex.slice(offset, offset + 2), 16) / 255,
  );
  return channels.reduce(
    (sum, value, index) =>
      sum +
      (value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4) *
        [0.2126, 0.7152, 0.0722][index]!,
    0,
  );
}

it.each(['light', 'dark'] as const)(
  'keeps all documented task text paints at AA contrast in %s',
  (theme) => {
    const palette = themes()[theme];
    const value = (name: string) =>
      resolveToken(
        name === 'secondary'
          ? 'dsw-alias-label-secondary'
          : name === 'error'
            ? 'dsw-alias-state-error-primary'
            : `dsw-specific-task-${name}`,
        palette,
      );
    const pairs = ['primary', 'weak', 'placeholder', 'secondary'].flatMap((text) =>
      ['page', 'shell', 'panel', 'menu'].map((ground) => [text, ground] as const),
    );
    pairs.push(
      ['on-accent', 'button-fill'],
      ['on-accent', 'accent-hover'],
      ['error', 'panel'],
      ['error', 'shell'],
    );
    for (const [text, ground] of pairs) {
      const levels = [luminance(value(text)), luminance(value(ground))].sort((a, b) => a - b);
      expect(
        (levels[1]! + 0.05) / (levels[0]! + 0.05),
        `${text} on ${ground}`,
      ).toBeGreaterThanOrEqual(4.5);
    }
  },
);

it.each(['light', 'dark'] as const)(
  'keeps app text, icons and state indicators accessible in %s',
  (theme) => {
    const palette = themes()[theme];
    const contrast = (foreground: string, background: string) => {
      const levels = [foreground, background]
        .map((token) => luminance(resolveToken(token, palette)))
        .sort((a, b) => a - b);
      return (levels[1]! + 0.05) / (levels[0]! + 0.05);
    };
    const grounds = [
      'dsw-alias-bg-base',
      'dsw-specific-sidebar-fill',
      'dsw-alias-bg-layer-1',
      'dsw-alias-bg-layer-2',
      'dsw-alias-bg-layer-3',
      'dsw-alias-interactive-bg-active',
      'dsw-specific-menu',
    ];
    for (const ground of grounds) {
      for (const rank of ['primary', 'secondary', 'tertiary'])
        expect(
          contrast(`dsw-alias-label-${rank}`, ground),
          `${rank} on ${ground}`,
        ).toBeGreaterThanOrEqual(4.5);
      // Light selected text uses primary, never the accent; its icon and ring still use accent.
      const selected = ['dsw-alias-bg-layer-3', 'dsw-alias-interactive-bg-active'].includes(ground);
      expect(
        contrast(selected ? 'dsw-alias-label-primary' : 'dsw-alias-brand-text', ground),
      ).toBeGreaterThanOrEqual(4.5);
      expect(contrast('dsw-alias-brand-primary', ground)).toBeGreaterThanOrEqual(3);
      expect(
        contrast('dsw-alias-label-tertiary', ground),
        'unselected radio indicator',
      ).toBeGreaterThanOrEqual(3);
    }
    for (const state of ['fill', 'hover', 'active'])
      expect(
        contrast('dsw-alias-button-primary-foreground', `dsw-alias-button-primary-${state}`),
      ).toBeGreaterThanOrEqual(4.5);
    // The Account Appearance and text-size choices and current sidebar labels use these exact roles.
    expect(
      contrast('dsw-alias-label-primary', 'dsw-specific-sidebar-nav-item-active'),
    ).toBeGreaterThanOrEqual(4.5);
  },
);

it('uses primary text and selected weight without link styling for menu choices and current sidebar labels', async () => {
  const { readFileSync } = await import('node:fs');
  const menu = readFileSync('src/ui/Menu.css', 'utf8');
  const sidebar = readFileSync('src/shell/Sidebar.css', 'utf8');
  expect(menu).toMatch(
    /\.menu__choice\[aria-checked='true'\]\s*\{[^}]*color: var\(--dsw-alias-label-primary\)[^}]*font-weight: 600/,
  );
  const selectedChoice = /\.menu__choice\[aria-checked='true'\]\s*\{([^}]*)\}/.exec(menu)?.[1];
  expect(selectedChoice).not.toMatch(/text-decoration/);
  expect(sidebar).toMatch(
    /\.sidebar__link\[aria-current='page'\]\s*\{[^}]*color: var\(--dsw-alias-label-primary\)/,
  );
  expect(sidebar).toMatch(
    /\.sidebar__link\[aria-current='page'\] > svg\s*\{[^}]*color: var\(--dsw-alias-brand-primary\)/,
  );
});

it.each(['light', 'dark'] as const)(
  'keeps canonical Usage, notice, chart and account paints readable in %s',
  (theme) => {
    const palette = themes()[theme];
    const contrast = (foreground: string, background: string) => {
      const levels = [foreground, background]
        .map((name) => luminance(resolveToken(name, palette)))
        .sort((a, b) => a - b);
      return (levels[1]! + 0.05) / (levels[0]! + 0.05);
    };
    for (const ground of ['dsw-alias-bg-layer-2', 'dsw-alias-bg-layer-1', 'dsw-specific-menu']) {
      for (const label of ['primary', 'secondary', 'tertiary'])
        expect(contrast(`dsw-alias-label-${label}`, ground)).toBeGreaterThanOrEqual(4.5);
      for (const mark of [
        'dsw-alias-brand-primary',
        'dsw-alias-label-secondary',
        'dsw-alias-state-error-primary',
      ])
        expect(contrast(mark, ground)).toBeGreaterThanOrEqual(3);
    }
    expect(
      contrast('dsw-alias-label-primary', 'dsw-specific-sidebar-nav-item-active'),
    ).toBeGreaterThanOrEqual(4.5);
  },
);
