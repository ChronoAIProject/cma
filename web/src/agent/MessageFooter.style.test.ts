import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { expect, it } from 'vitest';

// jsdom applies no hover or media rules, so the reveal contract is read from the sheet.
const css = readFileSync(resolve(process.cwd(), 'src/agent/Agent.css'), 'utf8');
const quiet = ['.message__timestamp'];
const block = (selector: RegExp) => css.match(selector)?.[0] ?? '';

it('keeps only time transparent at rest without removing it from layout', () => {
  const rest = block(/\.message__timestamp\s*\{\s*opacity[^}]*\}/);
  expect(rest).toMatch(/opacity: 0/);
  expect(rest).not.toMatch(/display: none|visibility: hidden/);
});

it('reveals time on hover and keyboard focus while action icons remain visible', () => {
  for (const trigger of ['.message:hover', '.message:focus-within'])
    expect(css).toContain(`${trigger} .message__timestamp`);
  expect(block(/\.message-footer__actions\s*\{[^}]*\}/)).not.toMatch(/opacity: 0/);
});

it('shows them at all times on devices that cannot hover', () => {
  const touch = block(/@media \(pointer: coarse\), \(hover: none\)[^{]*\{[^}]*\}/);
  for (const selector of quiet) expect(touch).toContain(selector);
  expect(touch).toMatch(/opacity: 1/);
});
