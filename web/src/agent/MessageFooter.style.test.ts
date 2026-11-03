import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { expect, it } from 'vitest';

// jsdom applies no hover or media rules, so the reveal contract is read from the sheet.
const css = readFileSync(resolve(process.cwd(), 'src/agent/Agent.css'), 'utf8');
const quiet = ['.message__timestamp', '.message__usage', '.message-footer__actions'];
const block = (selector: RegExp) => css.match(selector)?.[0] ?? '';

it('keeps time, usage and actions transparent at rest without removing them from layout', () => {
  const rest = block(
    /\.message__timestamp,\s*\.message__usage,\s*\.message-footer__actions\s*\{[^}]*\}/,
  );
  expect(rest).toMatch(/opacity: 0/);
  expect(rest).not.toMatch(/display: none|visibility: hidden/);
});

it('reveals them on hover and keyboard focus, and keeps a set vote visible', () => {
  for (const trigger of ['.message:hover', '.message:focus-within'])
    expect(css).toContain(
      `${trigger} :is(.message__timestamp, .message__usage, .message-footer__actions)`,
    );
  expect(css).toMatch(
    /\.message-footer\[data-voted\] \.message-footer__actions[^{]*\{\s*opacity: 1/,
  );
});

it('shows them at all times on devices that cannot hover', () => {
  const touch = block(/@media \(hover: none\)[^{]*\{[^}]*\}/);
  for (const selector of quiet) expect(touch).toContain(selector);
  expect(touch).toMatch(/opacity: 1/);
});
