import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { ToolCall } from './model';
import { ToolCallGroup, ToolCallRow } from './ToolCallRows';
import type { ToolGroup } from './timeline';

const call = (id: string, status: string, extra: Partial<ToolCall> = {}): ToolCall => ({
  id,
  runId: '1',
  title: `Call ${id}`,
  kind: 'execute',
  status,
  ...extra,
});
const group = (items: readonly ToolCall[], failed = false): ToolGroup => ({
  kind: 'tools',
  id: `tools:${items[0]!.id}`,
  items,
  failed,
});

describe('ToolCallRow', () => {
  it.each([
    ['completed', undefined, 'Completed', 'done'],
    ['failed', undefined, 'Failed', 'error'],
    ['in_progress', '1', 'In progress', 'ongoing'],
    ['in_progress', undefined, 'Outcome not reported (last status: in_progress)', 'neutral'],
  ])('pairs status %s with its word and dot', (status, activeRunId, word, dot) => {
    render(<ToolCallRow tool={call('a', status)} activeRunId={activeRunId} />);
    const row = screen.getByRole('article', { name: 'Tool call: Call a' });
    expect(row).toHaveTextContent(word);
    expect(row).toHaveTextContent('execute');
    expect(row.querySelector(`.statedot[data-state="${dot}"]`)).not.toBeNull();
  });

  it('draws no dot for a cancelled call and never states a duration', () => {
    render(<ToolCallRow tool={call('a', 'cancelled')} activeRunId={undefined} />);
    const row = screen.getByRole('article', { name: 'Tool call: Call a' });
    expect(row).toHaveTextContent('Cancelled');
    expect(row.querySelector('.statedot')).toBeNull();
    expect(row).not.toHaveTextContent(/duration|not reported/i);
  });

  it('offers Details only when the public contract supplied a body', () => {
    const { rerender } = render(<ToolCallRow tool={call('a', 'completed')} activeRunId="1" />);
    expect(screen.queryByText('Details')).not.toBeInTheDocument();
    rerender(<ToolCallRow tool={call('a', 'completed', { detail: 'exit 0' })} activeRunId="1" />);
    fireEvent.click(screen.getByText('Details'));
    expect(screen.getByText('exit 0')).toBeVisible();
  });
});

describe('ToolCallGroup', () => {
  it('starts collapsed with a count and the latest call, and toggles with a real button', () => {
    render(
      <ToolCallGroup
        group={group([call('a', 'completed'), call('b', 'completed', { title: 'Run tests' })])}
        activeRunId={undefined}
      />,
    );
    const toggle = screen.getByRole('button', {
      name: '2 tool calls Latest: Run tests · Completed',
    });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('article')).not.toBeInTheDocument();
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    const rows = document.getElementById(toggle.getAttribute('aria-controls')!)!;
    expect(
      within(rows)
        .getAllByRole('article')
        .map((row) => row.getAttribute('aria-label')),
    ).toEqual(['Tool call: Call a', 'Tool call: Run tests']);
    fireEvent.click(toggle);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toggle).not.toHaveAttribute('aria-controls');
  });

  it('opens a group holding a failure, including one that joins later, until the reader closes it', () => {
    const { rerender } = render(
      <ToolCallGroup
        group={group([call('a', 'completed'), call('b', 'completed')])}
        activeRunId={undefined}
      />,
    );
    const toggle = screen.getByRole('button', { name: /^2 tool calls/ });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    const failing = group(
      [call('a', 'completed'), call('b', 'completed'), call('c', 'failed')],
      true,
    );
    rerender(<ToolCallGroup group={failing} activeRunId={undefined} />);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(toggle).toHaveTextContent('3 tool calls');
    expect(screen.getByRole('article', { name: 'Tool call: Call c' })).toHaveTextContent('Failed');
    fireEvent.click(toggle);
    rerender(<ToolCallGroup group={failing} activeRunId={undefined} />);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
  });
});
