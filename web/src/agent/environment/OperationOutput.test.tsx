import { act, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { OperationOutput } from './OperationOutput';
import { AgentComputerObservation } from './observation';
import type { ToolCall } from '../model';
afterEach(() => vi.useRealTimers());
const command: ToolCall = {
  id: 'execute-1',
  kind: 'execute',
  title: 'printf "raw-command"',
  status: 'failed',
  output: { text: 'raw-output\nnext line', truncated: true },
};
it('keeps complete operation history visible and excludes non-execute tools', () => {
  render(
    <OperationOutput
      active
      workingDirectory="/home/cma/project"
      toolCalls={[{ id: 'read', kind: 'read', title: 'Read a file', status: 'completed' }, command]}
    />,
  );
  expect(screen.getByText('Operations')).toBeVisible();
  expect(screen.queryByText('bash')).not.toBeInTheDocument();
  expect(screen.getByText('/home/cma/project')).toBeVisible();
  expect(screen.queryByText('Read a file')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Operation name')).toHaveTextContent('printf "raw-command"');
  expect(screen.getByLabelText('Recorded output')).toBeVisible();
  expect(screen.getByLabelText('Recorded output').textContent).toBe('raw-output\nnext line');
  expect(screen.getByText('Output truncated')).toBeVisible();
  expect(screen.queryByText(/agent paused/i)).not.toBeInTheDocument();
  expect(screen.queryByRole('slider')).not.toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});
it('retains more than 50 operations without inventing a live cursor or paused task', () => {
  const tools = Array.from({ length: 60 }, (_, index) => ({
    ...command,
    id: `command-${index}`,
    title: `echo ${index}`,
  }));
  const { rerender } = render(<OperationOutput active activeRunId="1" toolCalls={tools} />);
  expect(screen.getAllByRole('article')).toHaveLength(60);
  expect(screen.queryByLabelText('Terminal cursor')).not.toBeInTheDocument();
  expect(screen.getAllByText('Failed')).toHaveLength(60);
  rerender(<OperationOutput active toolCalls={tools} />);
  expect(screen.getAllByRole('article')).toHaveLength(60);
  expect(screen.getAllByText('Failed')).toHaveLength(60);
});
it('jumps with a 16px inset and ends its highlight after 1.8s even as output arrives', async () => {
  vi.useFakeTimers();
  const observation = new AgentComputerObservation({ client: {}, subject: 'owner' });
  const view = (tools: readonly ToolCall[]) => (
    <div className="agent-computer__page">
      <OperationOutput active toolCalls={tools} observation={observation} />
    </div>
  );
  const { container, rerender } = render(view([command]));
  const port = container.firstElementChild as HTMLElement;
  const block = screen.getByRole('article');
  port.scrollTop = 40;
  vi.spyOn(port, 'getBoundingClientRect').mockReturnValue({ top: 20 } as DOMRect);
  vi.spyOn(block, 'getBoundingClientRect').mockReturnValue({ top: 230 } as DOMRect);
  vi.spyOn(
    container.querySelector('.workspace-terminal')!,
    'getBoundingClientRect',
  ).mockReturnValue({ bottom: 320 } as DOMRect);
  Object.defineProperty(port, 'clientHeight', { configurable: true, value: 600 });
  act(() => observation.selectTerminal(command.id));
  await act(() => vi.advanceTimersByTimeAsync(20));
  expect(port.scrollTop).toBe(234);
  expect(container.querySelector<HTMLElement>('.workspace-terminal')!.style.paddingBottom).toBe(
    '494px',
  );
  expect(block).toHaveFocus();
  expect(block).toHaveAttribute('data-highlighted', 'true');
  await act(() => vi.advanceTimersByTimeAsync(980));
  rerender(view([command, { ...command, id: 'second' }]));
  await act(() => vi.advanceTimersByTimeAsync(800));
  expect(block).not.toHaveAttribute('data-highlighted');
  expect(port.scrollTop).toBe(234);
});

it.each([
  ['completed', undefined, 'Completed'],
  ['failed', undefined, 'Failed'],
  ['cancelled', undefined, 'Cancelled'],
  ['in_progress', undefined, 'Outcome not reported (last status: in_progress)'],
  ['in_progress', 'other-turn', 'Outcome not reported (last status: in_progress)'],
  ['in_progress', 'tool-turn', 'In progress'],
] as const)(
  'reports execute-kind MCP status %s with active turn %s truthfully',
  (status, activeRunId, label) => {
    const tool: ToolCall = {
      id: 'mcp-call',
      runId: 'tool-turn',
      kind: 'execute',
      title: 'mcp.read_resource',
      detail: '{"resource":"<synthetic>"}',
      status,
    };
    const { container } = render(
      <OperationOutput active toolCalls={[tool]} activeRunId={activeRunId} />,
    );
    expect(screen.getByRole('article')).toHaveTextContent('mcp.read_resource');
    expect(screen.getByText(label)).toBeVisible();
    expect(screen.getByText('No output was recorded for this operation.')).toBeVisible();
    expect(screen.getByLabelText('Recorded details')).toHaveTextContent(tool.detail!);
    expect(container.textContent).not.toMatch(/bash|\$ |paused|● Live/);
    expect(container.querySelector('input, textarea, [contenteditable], synthetic')).toBeNull();
  },
);
