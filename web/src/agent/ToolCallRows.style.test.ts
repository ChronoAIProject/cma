import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { expect, it } from 'vitest';

const css = readFileSync(resolve(process.cwd(), 'src/agent/ToolCallRows.css'), 'utf8');
const rule = (selector: string) =>
  css.match(new RegExp(`${selector.replace(/[.[\]]/g, '\\$&')} \\{[^}]*\\}`))?.[0] ?? '';

it('draws tool activity as a line, not a box', () => {
  for (const selector of ['.tool-calls', '.agent-event--tool'])
    expect(rule(selector)).not.toMatch(/border|background|border-radius/);
});

it('hangs expanded rows against one left rule and keeps failures in error ink', () => {
  expect(rule('.tool-calls__rows')).toMatch(/border-left: 1px solid var\(--dsw-alias-border-l1\)/);
  expect(rule('.tool-calls__failed')).toMatch(/color: var\(--dsw-alias-state-error-primary\)/);
});
