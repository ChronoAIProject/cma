import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { CommandBar } from './CommandBar';
import type { Session } from '../sessions/types';
import { NO_CAPABILITIES } from '@chronoai/cma-sdk';

/**
 * The command bar — docs/Web_Style.md §6 and §12.
 *
 * The plate reaches the surfaces it can already reach and no more: sessions
 * by time and last-activity line, the two arrangements, new session, and the
 * appearance controls.
 */

const SESSION: Session = {
  id: 'ses_cmd',
  kernel: 'codex',
  lifecycle: 'running',
  createdAt: Date.parse('2026-08-18T08:53:00Z'),
  capabilities: NO_CAPABILITIES,
};

function renderBar(props: Partial<Parameters<typeof CommandBar>[0]> = {}) {
  const onClose = vi.fn();
  const onSelectSession = vi.fn();
  const onArrange = vi.fn();
  const onNewSession = vi.fn();
  const result = render(
    <CommandBar
      sessions={[SESSION]}
      creation={{ canCreate: true }}
      onClose={onClose}
      onSelectSession={onSelectSession}
      onArrange={onArrange}
      onNewSession={onNewSession}
      {...props}
    />,
  );
  return { ...result, onClose, onSelectSession, onArrange, onNewSession };
}

describe('the command bar', () => {
  it('uses the shared field geometry for its raw search control', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/shell/CommandBar.css'), 'utf8');
    expect(css).toMatch(/\.commandbar__field\s*\{[^}]*height: 44px[^}]*border-radius: 10px/);
    expect(css).toMatch(
      /\.commandbar__field\s*\{[^}]*background: var\(--dsw-alias-bg-layer-1\)[^}]*border: 1px solid var\(--dsw-alias-border-l2\)/,
    );
    expect(css).toMatch(
      /\.commandbar__field:focus-visible\s*\{[^}]*border-color: var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--dsw-ring-accent-soft\)/,
    );
  });

  it('draws the keyboard cursor and hover with accent ink on the accent wash', () => {
    const css = readFileSync(resolve(process.cwd(), 'src/shell/CommandBar.css'), 'utf8');
    expect(css).toMatch(
      /\.commandbar__item:hover:not\(:disabled\),\s*\.commandbar__item--active:not\(:disabled\)\s*\{[^}]*color: var\(--dsw-alias-state-business-primary\)[^}]*background: var\(--dsw-alias-interactive-bg-hover-accent\)/,
    );
  });

  it('reaches sessions, arrangements, new session and appearance — and no more', () => {
    renderBar();
    const labels = screen.getAllByRole('option').map((option) => option.textContent ?? '');

    expect(labels.some((label) => label.includes('New session'))).toBe(true);
    expect(labels.some((label) => label.startsWith('Session '))).toBe(true);
    expect(labels.some((label) => label.includes('list'))).toBe(true);
    expect(labels.some((label) => label.includes('canvas'))).toBe(true);
    expect(labels.filter((label) => label.includes('Appearance'))).toHaveLength(3);
    // Nothing the spec does not cover: every offer is one of the four kinds.
    for (const label of labels) {
      expect(
        label.includes('Session ') ||
          label.includes('New session') ||
          label.includes('Arrange') ||
          label.includes('Appearance'),
        label,
      ).toBe(true);
    }
  });

  it('names a session by time and by what it last did', () => {
    renderBar();
    const row = screen
      .getAllByRole('option')
      .find((option) => option.textContent?.startsWith('Session '));
    expect(row?.textContent).toContain('Running');
  });

  it('omits the canvas when the window has no room for it', () => {
    // §16 offers the canvas at 60rem and above; the bar lists nothing it
    // cannot do, so below that the entry is absent rather than disabled.
    renderBar({ canvasAvailable: false });
    const labels = screen.getAllByRole('option').map((option) => option.textContent ?? '');
    expect(labels.some((label) => label.includes('canvas'))).toBe(false);
  });

  it('filters as somebody types, and says what it reaches when nothing matches', async () => {
    const user = userEvent.setup();
    renderBar();

    await user.type(screen.getByRole('textbox'), 'new sess');
    expect(screen.getAllByRole('option')).toHaveLength(1);

    await user.clear(screen.getByRole('textbox'));
    await user.type(screen.getByRole('textbox'), 'zzzz');
    expect(screen.queryAllByRole('option')).toHaveLength(0);
    expect(screen.getByText(/reaches sessions, arrangements/i)).toBeInTheDocument();
  });

  it('runs the chosen command from the keyboard alone', async () => {
    const user = userEvent.setup();
    const { onNewSession, onClose } = renderBar();

    await user.type(screen.getByRole('textbox'), 'new session');
    await user.keyboard('{Enter}');

    expect(onNewSession).toHaveBeenCalled();
    expect(onClose).toHaveBeenCalled();
  });

  it('keeps a refused new-session entry listed with its reason and cannot run it', async () => {
    const user = userEvent.setup();
    const reason = 'You have 3 of 3 sessions running. Pause or end one to start another.';
    const { onNewSession, onClose } = renderBar({
      creation: { canCreate: false, reason },
    });
    const row = screen
      .getAllByRole('option')
      .find((option) => option.textContent?.includes('New session'));
    expect(row).toBeDefined();
    expect(row).toHaveTextContent(reason);

    const command = within(row as HTMLElement).getByRole('button');
    expect(command).toBeDisabled();
    await user.click(command);
    await user.type(screen.getByRole('textbox'), 'new session');
    await user.keyboard('{Enter}');
    expect(onNewSession).not.toHaveBeenCalled();
    expect(onClose).not.toHaveBeenCalled();
  });

  it('closes on Escape and returns focus to the trigger', async () => {
    // The trap, Escape and the focus return are the shared plate machinery —
    // written once in ui/plate.tsx.
    const user = userEvent.setup();
    const outside = document.createElement('button');
    outside.textContent = 'trigger';
    document.body.appendChild(outside);
    outside.focus();

    const { onClose, unmount } = renderBar();
    expect(screen.getByRole('textbox')).toHaveFocus();

    await user.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalled();

    unmount();
    expect(outside).toHaveFocus();
    outside.remove();
  });
});
