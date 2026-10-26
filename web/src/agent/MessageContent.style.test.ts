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
