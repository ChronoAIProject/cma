import { fireEvent, render, screen, within } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import type { ToolCall } from './model';
import { conversationFromTranscript, foldAgentEvent } from './conversation';
import type { AgentEvent } from '../api/types';
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
      if (['completed', 'failed', 'cancelled'].includes(status))
        expect(row.querySelector('.tool-call__duration')).toHaveTextContent('Duration unavailable');
      else expect(row.querySelector('.tool-call__duration')).toBeNull();
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
    fireEvent.click(screen.getByRole('button', { name: 'Edited app.js Duration unavailable' }));
    fireEvent.click(within(screen.getByRole('article')).getByRole('button', { name: /app.js/ }));
    expect(screen.getByText('/home/cma/game/app.js')).toBeVisible();
    expect(screen.getByText('-old')).toHaveClass('tool-call__removed');
    expect(screen.getByText('+new')).toHaveClass('tool-call__added');
    expect(screen.getByText('Output truncated')).toBeVisible();
  });

  it('jumps to Operations without expanding or collapsing the command', () => {
    const open = vi.fn();
    const computer = new AgentComputerObservation({ client: {}, subject: 'owner' }, open);
    render(
      <ComputerContext value={computer}>
        <ToolCallRow tool={call('a', 'completed')} />
      </ComputerContext>,
    );
    fireEvent.click(screen.getByRole('button', { name: 'Ran 1 command Duration unavailable' }));
    const command = screen.getByRole('button', { name: /Call a/ });
    fireEvent.click(screen.getByRole('button', { name: 'View in Operations' }));
    expect(command).toHaveAttribute('aria-expanded', 'false');
    expect(computer.terminalSnapshot()?.id).toBe('a');
    expect(open).toHaveBeenCalledOnce();
    fireEvent.click(command);
    fireEvent.click(screen.getByRole('button', { name: 'View in Operations' }));
    expect(command).toHaveAttribute('aria-expanded', 'true');
  });
});

describe('ToolCallGroup', () => {
  it('starts collapsed, including current failures, and retains the explicit reader choice', () => {
    const { rerender } = render(
      <ToolCallGroup group={group([call('a', 'completed'), call('b', 'completed')])} />,
    );
    const toggle = screen.getByRole('button', { name: 'Ran 2 commands Duration unavailable' });
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(screen.queryByRole('article')).not.toBeInTheDocument();
    rerender(
      <ToolCallGroup
        group={group([call('a', 'completed'), call('b', 'failed')])}
        activeRunId="1"
      />,
    );
    expect(toggle).toHaveAttribute('aria-expanded', 'false');
    expect(toggle).toHaveAccessibleName('Ran 2 commands Duration unavailable · 1 failed');
    fireEvent.click(toggle);
    const rows = document.getElementById(toggle.getAttribute('aria-controls')!)!;
    expect(within(rows).getAllByRole('article')).toHaveLength(2);
    rerender(<ToolCallGroup group={group([call('a', 'completed'), call('b', 'failed')])} />);
    expect(toggle).toHaveAttribute('aria-expanded', 'true');
    fireEvent.click(toggle);
    expect(toggle).not.toHaveAttribute('aria-controls');
  });

  it('keeps command names after sparse failure updates and accepts a recorded replacement title', () => {
    const appendedAt = '2026-11-05T14:33:00.000Z';
    const calls = [
      { id: 'a', title: 'Run tests', status: 'failed', update: '' },
      { id: 'b', title: 'Check formatting', status: 'failed', update: '   ' },
      { id: 'c', title: 'Read lockfile', status: 'completed', update: 'Verify lockfile' },
    ];
    const history: AgentEvent[] = calls.flatMap((entry, index) => [
      {
        seq: index * 2 + 1,
        appendedAt,
        kind: {
          type: 'tool_call_started',
          call_id: entry.id,
          kind: 'execute',
          title: entry.title,
          status: 'running',
        },
      },
      {
        seq: index * 2 + 2,
        appendedAt,
        kind: {
          type: 'tool_call_updated',
          call_id: entry.id,
          title: entry.update,
          status: entry.status,
        },
      },
    ]);
    const state = history.reduce(
      foldAgentEvent,
      conversationFromTranscript({ agentId: 'agt_1', throughSeq: 0, turns: [] }),
    );
    render(<ToolCallGroup group={group(state.toolCalls)} />);
    fireEvent.click(screen.getByRole('button', { name: /Ran 3 commands/ }));
    for (const name of ['Run tests', 'Check formatting', 'Verify lockfile']) {
      const row = screen.getByRole('article', { name: `Tool call: ${name}` });
      expect(within(row).getByRole('button', { name: new RegExp(name) })).toBeVisible();
    }
  });

  it('names a command card when the transcript has no title', () => {
    render(<ToolCallRow tool={call('legacy', 'failed', { title: ' ' })} />);
    fireEvent.click(screen.getByRole('button', { name: /Ran 1 command/ }));
    const row = screen.getByRole('article', { name: 'Tool call: Command' });
    expect(within(row).getByRole('button', { name: /^Command/ })).toBeVisible();
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
    ).toBe('Ran 1 command  Duration unavailable');
    expect(
      toolGroupSummary([
        call('1', 'completed', { startedAt: timed.finishedAt, finishedAt: timed.startedAt }),
      ]),
    ).toBe('Ran 1 command  Duration unavailable');
    expect(
      toolGroupSummary([
        call('edit', 'completed', {
          kind: 'edit',
          title: 'app.js',
          output: {
            text: '--- app.js\n+++ app.js\n@@ -1 +1,2 @@\n-old\n+one\n+two',
            truncated: false,
          },
        }),
      ]),
    ).toBe('Edited app.js +2 −1  Duration unavailable');
  });
});

it('shows a concise failure while keeping the full recorded output behind row expansion', () => {
  const output =
    'exit code 7\n' + 'large earlier output\n'.repeat(500) + 'last one\nlast two\nlast three';
  render(
    <ToolCallRow
      tool={call('failure', 'failed', { output: { text: output, truncated: false } })}
    />,
  );
  fireEvent.click(screen.getByRole('button', { name: /Ran 1 command/ }));
  const row = screen.getByRole('article');
  expect(row).not.toHaveTextContent('large earlier output');
  expect(row).toHaveTextContent('Exit code 7');
  expect(row).toHaveTextContent('last three');
  fireEvent.click(within(row).getByRole('button', { name: /Call failure/ }));
  expect(screen.getByLabelText('Recorded output')).toHaveTextContent('large earlier output');
});
