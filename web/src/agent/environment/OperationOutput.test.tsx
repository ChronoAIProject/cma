import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { OperationOutput } from './OperationOutput';

it('keeps command details folded while outcomes remain visible and preserves exact output', () => {
  render(
    <OperationOutput
      active
      toolCalls={[
        {
          id: 'read-1',
          kind: 'read',
          title: 'Read a file',
          status: 'completed',
        },
        {
          id: 'execute-1',
          kind: 'execute',
          title: 'printf "raw-command"',
          status: 'failed',
          output: { text: 'raw-output\nnext line', truncated: true },
        },
      ]}
    />,
  );
  expect(screen.getByText('Failed')).toBeVisible();
  expect(screen.queryByText('Read a file')).not.toBeInTheDocument();
  const command = screen.getByLabelText('Recorded command');
  const output = screen.getByLabelText('Recorded output');
  expect(command).not.toBeVisible();
  expect(output).not.toBeVisible();
  fireEvent.click(screen.getByText('Command 1'));
  expect(command).toBeVisible();
  expect(output).toBeVisible();
  expect(output.textContent).toBe('raw-output\nnext line');
  expect(screen.getByText('Output truncated')).toBeVisible();
  fireEvent.click(screen.getByText('Command 1'));
  expect(output).not.toBeVisible();
});
