import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { expect, it } from 'vitest';

const sheet = (name: string) => readFileSync(resolve(process.cwd(), `src/agent/${name}`), 'utf8');
const AXIS = 'max(24px, calc((100% - 768px) / 2))';

it('centres the history, its sections and the composer on one 768px axis', () => {
  const agent = sheet('Agent.css');
  const composer = sheet('Composer.css');
  for (const [css, selector] of [
    [agent, String.raw`\.agent-surface__conversation > section`],
    [agent, String.raw`\.message-stream`],
    [composer, String.raw`\.agent-composer`],
  ] as const) {
    const rule = css.match(new RegExp(`(?:^|\\n)${selector} \\{[^}]*\\}`))?.[0] ?? '';
    expect(rule).toContain(AXIS);
  }
  // A percentage gutter let the axis grow with the screen.
  expect(agent + composer).not.toMatch(/padding:[^;]*\b15%/);
});
