import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ArrangementToggle } from './ArrangementToggle';

describe('ArrangementToggle', () => {
  it('offers both arrangements', () => {
    render(<ArrangementToggle arrangement="tabs" onChange={vi.fn()} />);

    expect(screen.getByRole('group', { name: /arrangement/i })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Tabs' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Canvas' })).toBeInTheDocument();
  });

  it('marks the current mode without relying on colour', () => {
    render(<ArrangementToggle arrangement="canvas" onChange={vi.fn()} />);

    expect(screen.getByRole('button', { name: 'Canvas' })).toHaveAttribute('aria-pressed', 'true');
    expect(screen.getByRole('button', { name: 'Tabs' })).toHaveAttribute('aria-pressed', 'false');
  });

  it('reports a change', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ArrangementToggle arrangement="tabs" onChange={onChange} />);

    await user.click(screen.getByRole('button', { name: 'Canvas' }));

    expect(onChange).toHaveBeenCalledWith('canvas');
  });

  it('still reports a press on the mode already showing', async () => {
    // Idempotent rather than a toggle: pressing "Tabs" while in tabs mode must
    // not flip to canvas.
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ArrangementToggle arrangement="tabs" onChange={onChange} />);

    await user.click(screen.getByRole('button', { name: 'Tabs' }));

    expect(onChange).toHaveBeenCalledWith('tabs');
  });

  it('is reachable by keyboard', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<ArrangementToggle arrangement="tabs" onChange={onChange} />);

    await user.tab();
    await user.tab();
    await user.keyboard('{Enter}');

    expect(onChange).toHaveBeenCalledWith('canvas');
  });
});
