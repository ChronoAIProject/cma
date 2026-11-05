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
it('keeps complete command scrollback visible and excludes non-terminal tools', () => {
  render(
    <OperationOutput
      active
      workingDirectory="/home/cma/project"
      toolCalls={[{ id: 'read', kind: 'read', title: 'Read a file', status: 'completed' }, command]}
    />,
  );
  expect(screen.getByText('bash')).toBeVisible();
  expect(screen.getByText('/home/cma/project')).toBeVisible();
  expect(screen.queryByText('Read a file')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Recorded command')).toHaveTextContent('$ printf "raw-command"');
  expect(screen.getByLabelText('Recorded output')).toBeVisible();
  expect(screen.getByLabelText('Recorded output').textContent).toBe('raw-output\nnext line');
  expect(screen.getByText('Output truncated')).toBeVisible();
  expect(screen.getByText('— agent paused —')).toBeVisible();
  expect(screen.queryByRole('slider')).not.toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});
it('retains more than 50 commands and switches the live cursor to the paused line', () => {
  const tools = Array.from({ length: 60 }, (_, index) => ({
    ...command,
    id: `command-${index}`,
    title: `echo ${index}`,
  }));
  const { rerender } = render(<OperationOutput active activeRunId="1" toolCalls={tools} />);
  expect(screen.getAllByRole('article')).toHaveLength(60);
  expect(screen.getByText('● Live')).toBeVisible();
  expect(screen.getByLabelText('Terminal cursor')).toBeVisible();
  rerender(<OperationOutput active toolCalls={tools} />);
  expect(screen.getAllByRole('article')).toHaveLength(60);
  expect(screen.getByText('— agent paused —')).toBeVisible();
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
  act(() => observation.selectTerminal(command.id));
  await act(() => vi.advanceTimersByTimeAsync(20));
  expect(port.scrollTop).toBe(234);
  expect(block).toHaveFocus();
  expect(block).toHaveAttribute('data-highlighted', 'true');
  await act(() => vi.advanceTimersByTimeAsync(980));
  rerender(view([command, { ...command, id: 'second' }]));
  await act(() => vi.advanceTimersByTimeAsync(800));
  expect(block).not.toHaveAttribute('data-highlighted');
  expect(port.scrollTop).toBe(234);
});
