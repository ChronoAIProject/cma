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
      ['on-accent', 'accent'],
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
