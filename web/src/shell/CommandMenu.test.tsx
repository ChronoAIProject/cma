import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { CommandMenu, filterCommands, useCommandShortcut, type Command } from './CommandMenu';

function commands(run = vi.fn()): Command[] {
  return [
    {
      id: 'a1',
      group: 'Agents',
      label: 'Prove lemma',
      meta: 'trureturing · lab',
      hint: 'Working',
      run,
    },
    {
      id: 'a2',
      group: 'Agents',
      label: 'Refactor tactics',
      meta: 'tactics · lab',
      hint: 'Ready',
      run,
    },
    { id: 's1', group: 'Sandboxes', label: 'lab', hint: 'Live', run },
    { id: 'go', group: 'Go to', label: 'Docs', run },
  ];
}

describe('filterCommands', () => {
  it('returns everything for an empty query', () => {
    expect(filterCommands(commands(), '  ')).toHaveLength(4);
  });

  it('matches every word across label, meta, hint and group', () => {
    expect(filterCommands(commands(), 'lab working').map(({ id }) => id)).toEqual(['a1']);
    expect(filterCommands(commands(), 'SANDBOXES').map(({ id }) => id)).toEqual(['s1']);
  });

  it('returns nothing when a word matches nothing', () => {
    expect(filterCommands(commands(), 'lemma nowhere')).toEqual([]);
  });

  it('caps the list so a large hierarchy stays scannable', () => {
    const many = Array.from({ length: 100 }, (_, index) => ({
      id: String(index),
      group: 'Agents',
      label: `Agent ${String(index)}`,
      run: vi.fn(),
    }));
    expect(filterCommands(many, '')).toHaveLength(60);
  });
});

describe('CommandMenu', () => {
  it('focuses the field, filters as you type and runs the chosen command', async () => {
    const run = vi.fn();
    const onClose = vi.fn();
    const actor = userEvent.setup();
    render(<CommandMenu open onClose={onClose} commands={commands(run)} />);
    const field = screen.getByRole('combobox', { name: 'Search commands' });
    expect(field).toHaveFocus();
    expect(screen.getByRole('group', { name: 'Agents' })).toBeInTheDocument();
    await actor.type(field, 'tactics');
    const options = screen.getAllByRole('option');
    expect(options).toHaveLength(1);
    expect(options[0]).toHaveTextContent('Refactor tactics');
    expect(options[0]).toHaveAttribute('aria-selected', 'true');
    await actor.keyboard('{Enter}');
    expect(onClose).toHaveBeenCalledOnce();
    expect(run).toHaveBeenCalledOnce();
  });

  it('moves the active option with the arrow keys and wraps', async () => {
    const actor = userEvent.setup();
    render(<CommandMenu open onClose={vi.fn()} commands={commands()} />);
    const field = screen.getByRole('combobox', { name: 'Search commands' });
    const active = () =>
      document.getElementById(field.getAttribute('aria-activedescendant') ?? '')?.textContent;
    expect(active()).toContain('Prove lemma');
    await actor.keyboard('{ArrowDown}');
    expect(active()).toContain('Refactor tactics');
    await actor.keyboard('{ArrowUp}{ArrowUp}');
    expect(active()).toContain('Docs');
    await actor.keyboard('{Home}');
    expect(active()).toContain('Prove lemma');
    await actor.keyboard('{End}');
    expect(active()).toContain('Docs');
  });

  it('says so when nothing matches and runs nothing on Enter', async () => {
    const run = vi.fn();
    const actor = userEvent.setup();
    render(<CommandMenu open onClose={vi.fn()} commands={commands(run)} />);
    await actor.type(screen.getByRole('combobox', { name: 'Search commands' }), 'zzz');
    expect(screen.getByRole('status')).toHaveTextContent('No results.');
    await actor.keyboard('{Enter}');
    expect(run).not.toHaveBeenCalled();
  });

  it('runs a command chosen with the pointer', async () => {
    const run = vi.fn();
    const actor = userEvent.setup();
    render(<CommandMenu open onClose={vi.fn()} commands={commands(run)} />);
    const sandboxes = screen.getByRole('group', { name: 'Sandboxes' });
    await actor.click(within(sandboxes).getByRole('option', { name: /lab/ }));
    expect(run).toHaveBeenCalledOnce();
  });

  it('closes on Escape', async () => {
    const onClose = vi.fn();
    const actor = userEvent.setup();
    render(<CommandMenu open onClose={onClose} commands={commands()} />);
    await actor.keyboard('{Escape}');
    expect(onClose).toHaveBeenCalled();
  });

  it('draws nothing while closed', () => {
    render(<CommandMenu open={false} onClose={vi.fn()} commands={commands()} />);
    expect(screen.queryByRole('dialog')).toBeNull();
  });
});

describe('useCommandShortcut', () => {
  function Harness({ enabled = true }: { readonly enabled?: boolean }) {
    const [count, setCount] = useState(0);
    useCommandShortcut(() => setCount((value) => value + 1), enabled);
    return <output>{count}</output>;
  }

  it('toggles on Cmd+K and Ctrl+K', () => {
    render(<Harness />);
    fireEvent.keyDown(window, { key: 'k', metaKey: true });
    fireEvent.keyDown(window, { key: 'K', ctrlKey: true });
    expect(screen.getByRole('status')).toHaveTextContent('2');
  });

  it('ignores plain K, other chords and IME composition', () => {
    render(<Harness />);
    fireEvent.keyDown(window, { key: 'k' });
    fireEvent.keyDown(window, { key: 'k', metaKey: true, shiftKey: true });
    fireEvent.keyDown(window, { key: 'j', metaKey: true });
    fireEvent.keyDown(window, { key: 'k', metaKey: true, isComposing: true });
    expect(screen.getByRole('status')).toHaveTextContent('0');
  });

  it('listens to nothing while disabled', () => {
    render(<Harness enabled={false} />);
    fireEvent.keyDown(window, { key: 'k', metaKey: true });
    expect(screen.getByRole('status')).toHaveTextContent('0');
  });
});
