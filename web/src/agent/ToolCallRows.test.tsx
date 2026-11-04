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
    const toggle = screen.getByRole('button', { name: /^1 tool call/ });
    if (toggle.getAttribute('aria-expanded') === 'false') fireEvent.click(toggle);
    const row = screen.getByRole('article', { name: 'Tool call: Call a' });
    expect(row).toHaveTextContent(word);
    expect(row).toHaveTextContent('execute');
    expect(row.querySelector(`.statedot[data-state="${dot}"]`)).not.toBeNull();
  });

  it('draws no dot for a cancelled call and never states a duration', () => {
    render(<ToolCallRow tool={call('a', 'cancelled')} activeRunId={undefined} />);
    fireEvent.click(screen.getByRole('button', { name: '1 tool call' }));
    const row = screen.getByRole('article', { name: 'Tool call: Call a' });
    expect(row).toHaveTextContent('Cancelled');
    expect(row.querySelector('.statedot')).toBeNull();
    expect(row).not.toHaveTextContent(/duration|not reported/i);
  });

  it('offers Details only when the public contract supplied a body', () => {
    const { rerender } = render(<ToolCallRow tool={call('a', 'completed')} activeRunId="1" />);
    fireEvent.click(screen.getByRole('button', { name: '1 tool call' }));
    expect(screen.queryByText('Details')).not.toBeInTheDocument();
    rerender(<ToolCallRow tool={call('a', 'completed', { detail: 'exit 0' })} activeRunId="1" />);
    fireEvent.click(screen.getByText('Details'));
    expect(screen.getByText('exit 0')).toBeVisible();
  });
});

describe('ToolCallGroup', () => {
  it('starts collapsed with only a count when settled, and toggles with a real button', () => {
    render(
      <ToolCallGroup
        group={group([call('a', 'completed'), call('b', 'completed', { title: 'Run tests' })])}
        activeRunId={undefined}
      />,
    );
    // A settled group repeats no command line.
    const toggle = screen.getByRole('button', { name: '2 tool calls' });
    expect(toggle).not.toHaveTextContent(/Latest|Run tests|Completed/);
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

  it('opens failures in the current turn until the reader closes them', () => {
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
    rerender(<ToolCallGroup group={failing} activeRunId="1" />);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    expect(toggle).toHaveAccessibleName('3 tool calls · 1 failed');
    expect(toggle.querySelector('.tool-calls__failed')).toHaveTextContent('· 1 failed');
    expect(screen.getByRole('article', { name: 'Tool call: Call c' })).toHaveTextContent('Failed');
    fireEvent.click(toggle);
    rerender(<ToolCallGroup group={failing} activeRunId={undefined} />);
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
  });

  it('names the latest running call while the turn is active, and drops it once settled', () => {
    const running = group([
      call('a', 'completed'),
      call('b', 'in_progress', { title: 'Run tests' }),
      call('c', 'in_progress', { title: 'Build image' }),
    ]);
    const { rerender } = render(<ToolCallGroup group={running} activeRunId="1" />);
    expect(
      screen.getByRole('button', { name: '3 tool calls · Running Build image' }),
    ).toBeVisible();
    // The same pending calls after their turn ended are not running any more.
    rerender(<ToolCallGroup group={running} activeRunId={undefined} />);
    expect(screen.getByRole('button', { name: '3 tool calls' })).toBeVisible();
  });

  it('keeps completed operation details collapsed while reporting every failure', () => {
    render(
      <ToolCallGroup
        group={group([call('a', 'failed'), call('b', 'completed'), call('c', 'error')], true)}
        activeRunId={undefined}
      />,
    );
    expect(screen.getByRole('button', { name: '3 tool calls · 2 failed' })).toHaveAttribute(
      'aria-expanded',
      'false',
    );
  });
});
