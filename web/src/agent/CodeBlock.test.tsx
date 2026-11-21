import { act, render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { MessageContent } from './MessageContent';

afterEach(() => {
  vi.restoreAllMocks();
  vi.useRealTimers();
});

function clipboard(writeText: (text: string) => Promise<void>) {
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
}

describe('CodeBlock', () => {
  it('labels a fenced block with its info string and copies the code exactly', async () => {
    const user = userEvent.setup();
    const writeText = vi.fn(() => Promise.resolve());
    clipboard(writeText);
    const { container } = render(<MessageContent content={'```bash\nnpm run build\n```'} />);
    expect(container.querySelector('.code-block__language')).toHaveTextContent('bash');
    await user.click(screen.getByRole('button', { name: 'Copy code' }));
    expect(writeText).toHaveBeenCalledWith('npm run build');
    expect(await screen.findByText('Code copied')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy code' })).toHaveAttribute('title', 'Copied');
  });

  it('returns to Copy after a moment', async () => {
    clipboard(() => Promise.resolve());
    render(<MessageContent content={'```\nls\n```'} />);
    vi.useFakeTimers();
    await act(async () => {
      screen.getByRole('button', { name: 'Copy code' }).click();
      await Promise.resolve();
    });
    expect(screen.getByText('Code copied')).toBeInTheDocument();
    act(() => vi.advanceTimersByTime(2000));
    expect(screen.queryByText('Code copied')).not.toBeInTheDocument();
  });

  it('draws no language label for a fence without an info string', () => {
    const { container } = render(<MessageContent content={'```\nplain\n```'} />);
    expect(container.querySelector('.code-block__language')).toBeNull();
    expect(container.querySelector('pre')).toHaveTextContent('plain');
  });

  it('reports a failed copy', async () => {
    const user = userEvent.setup();
    clipboard(() => Promise.reject(new Error('denied')));
    render(<MessageContent content={'```js\nconsole.log(1)\n```'} />);
    await user.click(screen.getByRole('button', { name: 'Copy code' }));
    await waitFor(() => {
      expect(screen.getByText('Copy failed')).toBeInTheDocument();
    });
  });

  it('reports a missing clipboard as a failed copy', async () => {
    const user = userEvent.setup();
    Object.defineProperty(navigator, 'clipboard', { configurable: true, value: undefined });
    render(<MessageContent content={'```\nls\n```'} />);
    await user.click(screen.getByRole('button', { name: 'Copy code' }));
    expect(await screen.findByText('Copy failed')).toBeInTheDocument();
  });

  it('holds Copy while the reply is still streaming', () => {
    render(<MessageContent content={'```sh\nrm -rf bu'} streaming />);
    const key = screen.getByRole('button', { name: 'Copy code' });
    expect(key).toBeDisabled();
    expect(key).toHaveAttribute('title', 'Available when the reply finishes');
  });
});
