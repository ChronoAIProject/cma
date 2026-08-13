import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { SessionTabs } from './SessionTabs';
import type { Session } from './types';

const SESSIONS: readonly Session[] = [
  { id: 's-0001', kernel: 'codex', lifecycle: 'running', createdAt: 0 },
  { id: 's-0002', kernel: 'claude-code', lifecycle: 'idle', createdAt: 0 },
  { id: 's-0003', kernel: 'codex', lifecycle: 'terminated', createdAt: 0 },
];

describe('SessionTabs', () => {
  it('lists every session', () => {
    render(
      <SessionTabs sessions={SESSIONS} activeId="s-0001" onSelect={vi.fn()} onCreate={vi.fn()} />,
    );

    expect(screen.getAllByRole('tab')).toHaveLength(3);
  });

  it('always pairs the status dot with a word', () => {
    render(
      <SessionTabs sessions={SESSIONS} activeId="s-0001" onSelect={vi.fn()} onCreate={vi.fn()} />,
    );

    // Web_Style.md forbids conveying state through colour alone.
    expect(screen.getByText('running')).toBeInTheDocument();
    expect(screen.getByText('idle')).toBeInTheDocument();
    expect(screen.getByText('ended')).toBeInTheDocument();
  });

  it('marks the active session for assistive technology', () => {
    render(
      <SessionTabs sessions={SESSIONS} activeId="s-0002" onSelect={vi.fn()} onCreate={vi.fn()} />,
    );

    const active = screen.getAllByRole('tab').find((t) => t.textContent?.includes('s-0002'));
    expect(active).toHaveAttribute('aria-selected', 'true');
  });

  it('switches session on click', async () => {
    const user = userEvent.setup();
    const onSelect = vi.fn();
    render(
      <SessionTabs sessions={SESSIONS} activeId="s-0001" onSelect={onSelect} onCreate={vi.fn()} />,
    );

    const tab = screen.getAllByRole('tab').find((t) => t.textContent?.includes('s-0002'));
    await user.click(tab as HTMLElement);

    expect(onSelect).toHaveBeenCalledWith('s-0002');
  });

  it('spawns a session from the + control', async () => {
    const user = userEvent.setup();
    const onCreate = vi.fn();
    render(
      <SessionTabs sessions={SESSIONS} activeId="s-0001" onSelect={vi.fn()} onCreate={onCreate} />,
    );

    await user.click(screen.getByRole('button', { name: /new session/i }));

    expect(onCreate).toHaveBeenCalledOnce();
  });

  it('renders with no sessions rather than crashing', () => {
    render(<SessionTabs sessions={[]} activeId={null} onSelect={vi.fn()} onCreate={vi.fn()} />);

    expect(screen.queryAllByRole('tab')).toHaveLength(0);
    expect(screen.getByRole('button', { name: /new session/i })).toBeInTheDocument();
  });

  it('names the strip for assistive technology', () => {
    render(
      <SessionTabs sessions={SESSIONS} activeId={null} onSelect={vi.fn()} onCreate={vi.fn()} />,
    );

    expect(screen.getByRole('tablist', { name: /sessions/i })).toBeInTheDocument();
  });
});
