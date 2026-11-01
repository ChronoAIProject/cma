import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { expect, it } from 'vitest';

const css = readFileSync(resolve(process.cwd(), 'src/agent/MessageContent.css'), 'utf8');

it('keeps inline code tight and relative to its text, and code blocks framed', () => {
  const inline = css.match(/\.agent-content :not\(pre\) > code \{[^}]*\}/)?.[0] ?? '';
  expect(inline).toMatch(/padding: 1px 4px/);
  expect(inline).toMatch(/font-size: 0\.875em/);
  expect(inline).not.toMatch(/border:/);
  expect(css).toMatch(/\.agent-content pre \{[^}]*border: 1px solid/);
});

it('underlines reply links at rest, so they never rely on colour alone', () => {
  const link = css.match(/\.agent-content a \{[^}]*\}/)?.[0] ?? '';
  expect(link).toMatch(/text-decoration: underline/);
  expect(link).not.toMatch(/transition: all/);
});

it('sets one reading size for a reply and quiet, rule-only tables', () => {
  expect(css).toMatch(/\.agent-content \{[^}]*font: var\(--dsw-font-markdown-base\)/);
  const cells = css.match(/\.agent-content th,\n\.agent-content td \{[^}]*\}/)?.[0] ?? '';
  expect(cells).toMatch(/border-bottom: 1px solid/);
  expect(cells).not.toMatch(/border: 1px/);
  expect(css).toMatch(/\.agent-content table \{[^}]*font-variant-numeric: tabular-nums/);
});

it('keeps a reply heading below the page title and plain wrapping while it streams', () => {
  expect(css).toMatch(/\.agent-content h2 \{[^}]*font: var\(--dsw-font-markdown-h3\)/);
  expect(css).toMatch(/\.agent-content\[data-streaming\] :is\(p, li\) \{[^}]*text-wrap: wrap/);
});
