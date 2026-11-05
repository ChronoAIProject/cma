import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { ToolCall } from './model';
import { ToolCallGroup, ToolCallRow } from './ToolCallRows';
import type { ToolGroup } from './timeline';
import { AgentComputerObservation } from './environment/observation';
import { ComputerContext } from './environment/context';
import { toolGroupSummary } from './toolSummary';

const call = (id: string, status: string, extra: Partial<ToolCall> = {}): ToolCall => ({
  id,
  runId: '1',
  title: `Call ${id}`,
  kind: 'execute',
  status,
  ...extra,
});
const group = (items: readonly ToolCall[]): ToolGroup => ({
  kind: 'tools',
  id: `tools:${items[0]!.id}`,
  items,
  failed: items.some((tool) => tool.status === 'failed'),
});

describe('ToolCallRow', () => {
  it.each([
    ['completed', undefined, 'Completed', '✓'],
    ['failed', undefined, 'Failed', '!'],
    ['in_progress', '1', 'In progress', '·'],
    ['in_progress', undefined, 'Outcome not reported (last status: in_progress)', '·'],
    ['cancelled', undefined, 'Cancelled', '■'],
  ])(
    'keeps the outcome accessible without repeating visible status text: %s',
    (status, activeRunId, label, symbol) => {
      render(<ToolCallRow tool={call('a', status)} activeRunId={activeRunId} />);
      fireEvent.click(screen.getByRole('button', { name: /Ran 1 command/ }));
      const row = screen.getByRole('article', { name: 'Tool call: Call a' });
      expect(row.querySelector('.tool-call__outcome')).toHaveAttribute('aria-label', label);
      expect(row.querySelector('.tool-call__outcome')).toHaveTextContent(symbol);
      expect(row).not.toHaveTextContent('execute');
      expect(row).not.toHaveTextContent('Completed');
      if (status === 'failed') expect(row).toHaveTextContent('The operation failed.');
      expect(row.querySelector('.tool-call__duration')).toBeNull();
    },
  );

  it('expands the entire literal and recorded diff, keeping output bounds visible', () => {
    render(
      <ToolCallRow
        tool={call('a', 'completed', {
          kind: 'edit',
          title: 'app.js',
          detail: '/home/cma/game/app.js',
          output: { text: '-old\n+new', truncated: true },
        })}
      />,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Edited app.js' }));
    fireEvent.click(within(screen.getByRole('article')).getByRole('button', { name: /app.js/ }));
    expect(screen.getByText('/home/cma/game/app.js')).toBeVisible();
    expect(screen.getByText('-old')).toHaveClass('tool-call__removed');
    expect(screen.getByText('+new')).toHaveClass('tool-call__added');
    expect(screen.getByText('Output truncated')).toBeVisible();
  });

  it('jumps to Terminal without expanding or collapsing the command', () => {
    const open = vi.fn();
    const computer = new AgentComputerObservation({ client: {}, subject: 'owner' }, open);
    render(
      <ComputerContext value={computer}>
        <ToolCallRow tool={call('a', 'completed')} />
      </ComputerContext>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Ran 1 command' }));
    const command = screen.getByRole('button', { name: /Call a/ });
    fireEvent.click(screen.getByRole('button', { name: 'View in Terminal' }));
    expect(command).toHaveAttribute('aria-expanded', 'false');
    expect(computer.terminalSnapshot()?.id).toBe('a');
    expect(open).toHaveBeenCalledOnce();
    fireEvent.click(command);
    fireEvent.click(screen.getByRole('button', { name: 'View in Terminal' }));
    expect(command).toHaveAttribute('aria-expanded', 'true');
  });
});

describe('ToolCallGroup', () => {
  it('starts collapsed, including current failures, and retains the explicit reader choice', () => {
    const { rerender } = render(
      <ToolCallGroup group={group([call('a', 'completed'), call('b', 'completed')])} />,
    );
    const toggle = screen.getByRole('button', { name: 'Ran 2 commands' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('article')).not.toBeInTheDocument();
    rerender(
      <ToolCallGroup
        group={group([call('a', 'completed'), call('b', 'failed')])}
        activeRunId="1"
      />,
    );
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toggle).toHaveAccessibleName('Ran 2 commands · 1 failed');
    fireEvent.click(toggle);
    const rows = document.getElementById(toggle.getAttribute('aria-controls')!)!;
    expect(within(rows).getAllByRole('article')).toHaveLength(2);
    rerender(<ToolCallGroup group={group([call('a', 'completed'), call('b', 'failed')])} />);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    fireEvent.click(toggle);
    expect(toggle).not.toHaveAttribute('aria-controls');
  });

  it('summarizes commands, reads and edits with only valid recorded durations', () => {
    const timed = { startedAt: '2026-11-05T14:33:00.000Z', finishedAt: '2026-11-05T14:33:00.550Z' };
    const tools = [
      call('1', 'completed', timed),
      call('2', 'completed', timed),
      call('3', 'completed', { ...timed, kind: 'read' }),
      call('4', 'completed', { ...timed, kind: 'read' }),
    ];
    expect(toolGroupSummary(tools)).toBe('Ran 2 commands · Read 2 files  2.2s');
    expect(
      toolGroupSummary([
        call('1', 'completed', { startedAt: 'invalid', finishedAt: timed.finishedAt }),
      ]),
    ).toBe('Ran 1 command');
    expect(
      toolGroupSummary([
        call('1', 'completed', { startedAt: timed.finishedAt, finishedAt: timed.startedAt }),
      ]),
    ).toBe('Ran 1 command');
    expect(
      toolGroupSummary([
        call('edit', 'completed', {
          kind: 'edit',
          title: 'app.js',
          output: { text: '--- app.js\n+++ app.js\n-old\n+one\n+two', truncated: false },
        }),
      ]),
    ).toBe('Edited app.js +2 −1');
  });
});
