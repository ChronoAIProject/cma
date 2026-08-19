import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { DetailsPanel } from './DetailsPanel';

/**
 * The tool-call inspector — `docs/Web_Style.md` §12.
 *
 * What a call carries used to open inside its row, in the reading column. It
 * is unbounded, it is not prose, and it pushed the conversation it belongs to
 * off the screen. These pin the properties that made moving it worth doing.
 */

const SECTIONS = [
  { id: 'arguments', label: 'Arguments', body: '{"path":"/etc/hosts"}' },
  { id: 'result', label: 'Result', body: 'no such file', failed: true },
];

describe('the details panel', () => {
  it('names the call it is showing', () => {
    render(<DetailsPanel title="read /etc/hosts" sections={SECTIONS} onClose={vi.fn()} />);
    expect(screen.getByRole('heading', { name: 'read /etc/hosts' })).toBeInTheDocument();
  });

  it('falls back to a name rather than an empty header', () => {
    render(<DetailsPanel sections={SECTIONS} onClose={vi.fn()} />);
    expect(screen.getByRole('heading', { name: 'Details' })).toBeInTheDocument();
  });

  it('draws each body under its own label, in order', () => {
    // What a call was asked to do comes before what it produced, because that
    // is the order a reader reconstructs them in.
    render(<DetailsPanel title="read" sections={SECTIONS} onClose={vi.fn()} />);
    const labels = [...document.querySelectorAll('.details__label')].map((el) => el.textContent);
    expect(labels).toEqual(['Arguments', 'Result']);
  });

  it('says which body was cut rather than that something was', () => {
    render(
      <DetailsPanel
        title="read"
        sections={[{ id: 'result', label: 'Result', body: 'x', truncated: true }]}
        onClose={vi.fn()}
      />,
    );
    expect(screen.getByText('Result (truncated)')).toBeInTheDocument();
  });

  it('marks the body that went wrong', () => {
    render(<DetailsPanel title="read" sections={SECTIONS} onClose={vi.fn()} />);
    const blocks = [...document.querySelectorAll('.details__block')];
    expect(blocks[0]?.hasAttribute('data-error')).toBe(false);
    expect(blocks[1]?.hasAttribute('data-error')).toBe(true);
  });

  it('says so when a call carries nothing, rather than drawing an empty panel', () => {
    // An absence is a fact. A blank column reads as something that failed to
    // load, which is a different claim.
    render(<DetailsPanel title="read" sections={[]} onClose={vi.fn()} />);
    expect(screen.getByText(/nothing to show/i)).toBeInTheDocument();
  });

  it('wraps a long line rather than scrolling it sideways', () => {
    /*
     * The column is as wide as somebody dragged it. A horizontal scrollbar
     * under every block would be a second gesture for reading one thing, and
     * the end of a line would be hidden behind it.
     */
    const css = readFileSync(resolve(process.cwd(), 'src/chat/DetailsPanel.css'), 'utf8');
    const block = /\.details__block\s*\{([^}]*)\}/.exec(css)?.[1] ?? '';
    expect(block).toContain('white-space: pre-wrap');
    expect(block).toContain('overflow-wrap: anywhere');
    expect(block).not.toContain('overflow-x');
  });

  it('closes when asked', async () => {
    const user = userEvent.setup();
    const onClose = vi.fn();
    render(<DetailsPanel title="read" sections={SECTIONS} onClose={onClose} />);

    await user.click(screen.getByRole('button', { name: 'Close details' }));
    expect(onClose).toHaveBeenCalledOnce();
  });
});
