import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it } from 'vitest';
import { screenOperation, screenApplication } from './screenOperations';
import { groupToolCalls } from './timeline';
import { ToolCallGroup } from './ToolCallRows';
import type { ToolCall } from './model';
const tool = (title: string, more: Partial<ToolCall> = {}): ToolCall => ({
  id: title,
  title,
  kind: 'execute',
  status: 'completed',
  startedAt: '2026-11-05T06:33:02Z',
  ...more,
});
it.each([
  ['cma-browser navigate http://localhost:5173', 'navigate', 'Browser'],
  [`cma-browser input '{"kind":"click","x":40,"y":20}'`, 'click', 'Browser'],
  [`cma-browser input '{"kind":"text","text":"private input"}'`, 'type', 'Browser'],
  ['xdotool mousemove 20 40 click 1', 'click', 'Desktop'],
  ['xdotool mousemove 20 40 mousedown 1 mousemove 80 90 mouseup 1', 'drag', 'Desktop'],
])('derives only reported actions from %s', (title, action, application) => {
  expect(screenOperation(tool(title))).toMatchObject({ action, application });
});
it.each([
  'echo "cma-browser navigate http://example.test"',
  'xdotool search --name click',
  'cma-browser navigate $TARGET',
  'cma-browser navigate http://example.test && echo done',
  'fetch a page',
])('retains ambiguous or non-screen titles as ordinary tools: %s', (title) => {
  expect(screenOperation(tool(title))).toBeUndefined();
});
it('separates consecutive screen operations and shows noninteractive descriptions and recorded times', () => {
  const steps = [
    tool('cma-browser navigate http://localhost:5173'),
    tool(`cma-browser input '{"kind":"click","x":40,"y":20}'`),
    tool(`cma-browser input '{"kind":"text","text":"private input"}'`, {
      finishedAt: '2026-11-05T06:33:21Z',
    }),
  ];
  const timeline = groupToolCalls(
    [tool('pwd'), ...steps, tool('git status')].map((item) => ({ kind: 'tool', item })),
  );
  expect(timeline.map((item) => item.kind)).toEqual(['tool', 'tools', 'tool']);
  render(<ToolCallGroup group={{ kind: 'tools', id: 'screens', failed: false, items: steps }} />);
  const toggle = screen.getByRole('button', { name: 'Checked it in Browser · 3 steps 19s' });
  expect(toggle).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(toggle);
  const rows = screen.getAllByRole('article');
  expect(rows).toHaveLength(3);
  for (const row of rows) {
    expect(row.querySelector('time')).toHaveAttribute('datetime', '2026-11-05T06:33:02Z');
    expect(row.querySelector('time')?.textContent).toMatch(/^\d\d:\d\d:\d\d$/);
    expect(within(row).queryByRole('button')).not.toBeInTheDocument();
  }
  expect(screen.queryByText('private input')).not.toBeInTheDocument();
  expect(screen.queryByRole('img')).not.toBeInTheDocument();
});

it.each(['browser', 'computer'])(
  'does not invent a screen producer for unsupported kind %s',
  (kind) => {
    expect(screenOperation(tool('Click the page', { kind }))).toBeUndefined();
  },
);

it('follows desktop window commands while retaining them as ordinary tool rows', () => {
  for (const title of ['xdotool windowactivate 44', 'wmctrl -a Browser']) {
    expect(screenApplication(tool(title))).toBe('Desktop');
    expect(screenOperation(tool(title))).toBeUndefined();
  }
});
