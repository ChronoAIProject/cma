import { fireEvent, render, screen, within } from '@testing-library/react';
import { expect, it } from 'vitest';
import { ToolCallGroup } from './ToolCallRows';
import { OperationOutput } from './environment/OperationOutput';
import type { ToolCall } from './model';

const start = '2026-11-05T14:33:00.000Z';
const timed: ToolCall = {
  id: 'timed',
  title: 'Check fixture',
  kind: 'execute',
  status: 'completed',
  startedAt: start,
  finishedAt: '2026-11-05T14:33:00.020Z',
};
const unknown: ToolCall = {
  ...timed,
  id: 'unknown',
  title: 'Read fixture',
  kind: 'read',
  finishedAt: start,
};

it('shows unavailable group timing while retaining known row timing and successful outcomes', () => {
  render(
    <ToolCallGroup
      group={{ kind: 'tools', id: 'tools:timed', items: [timed, unknown], failed: false }}
    />,
  );
  const toggle = screen.getByRole('button', {
    name: /Ran 1 command · Read 1 file.*Duration unavailable/,
  });
  fireEvent.click(toggle);
  const known = screen.getByRole('article', { name: 'Tool call: Check fixture' });
  const missing = screen.getByRole('article', { name: 'Tool call: Read fixture' });
  expect(within(known).getByText('<0.1s')).toBeVisible();
  expect(within(missing).getByText('Duration unavailable')).toBeVisible();
  for (const row of [known, missing])
    expect(within(row).getByRole('img', { name: 'Completed' })).toBeVisible();
  expect(document.body).not.toHaveTextContent('0.0s');
});

it('uses the same recorded timing in Operations without changing tool outcomes', () => {
  render(<OperationOutput active toolCalls={[timed, { ...unknown, kind: 'execute' }]} />);
  expect(screen.getByText('<0.1s')).toBeVisible();
  expect(screen.getByText('Duration unavailable')).toBeVisible();
  expect(screen.getAllByText('Completed')).toHaveLength(2);
});
