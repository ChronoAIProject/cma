import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { expect, it } from 'vitest';
const css = readFileSync(resolve(process.cwd(), 'src/agent/ToolCallRows.css'), 'utf8');
it('uses the documented Workspace row, mono, border and failure tokens', () => {
  for (const token of [
    'workspace-row-height',
    'workspace-tool-font',
    'workspace-border',
    'workspace-danger',
  ])
    expect(css).toContain(`var(--dsw-alias-${token})`);
  expect(css).not.toMatch(/#[a-f0-9]{3,8}\b/i);
  expect(css).toContain(':focus-visible');
  expect(css).toContain('(pointer: coarse)');
});
