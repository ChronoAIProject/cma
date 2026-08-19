import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Pill } from './Pill';
import { StateDot } from './StateDot';

/**
 * The badge and the state dot — `docs/Web_Style.md` §7.
 *
 * These replaced one component that was both at once, and the split is the
 * point: a label with no state behind it now has no dot to mis-colour, and a
 * state has a dot without being forced into a pill.
 */

const SRC = resolve(process.cwd(), 'src');

describe('the pill', () => {
  it('is a span until it is given something to do', () => {
    // A span that reacts to clicks is unreachable by keyboard and silent to a
    // screen reader; a button that never does anything announces every static
    // label as pressable. Which element it is has to follow the behaviour.
    const { container } = render(<Pill>required</Pill>);
    expect(container.firstElementChild?.tagName).toBe('SPAN');
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('is a button the moment it is', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(<Pill onClick={onClick}>Live</Pill>);

    const pill = screen.getByRole('button', { name: 'Live' });
    await user.click(pill);
    expect(onClick).toHaveBeenCalledOnce();
  });

  it('says whether an interactive pill is the selected one', () => {
    // `active` is drawn, and a drawing is not an announcement. Without the
    // state on the element, a selected filter is selected only for people who
    // can see the fill.
    render(
      <Pill active onClick={vi.fn()}>
        Live
      </Pill>,
    );
    expect(screen.getByRole('button', { name: 'Live' })).toHaveAttribute('aria-pressed', 'true');
  });

  it('marks selection without changing the box', () => {
    // An inset ring rather than a border: a real border would grow the pill
    // by 2px and shift its neighbours every time the selection moved along
    // the row.
    const css = readFileSync(join(SRC, 'ui', 'Pill.css'), 'utf8');
    expect(css).toMatch(/\.pill--active\s*\{[^}]*box-shadow: inset/);
    expect(css).not.toMatch(/\.pill--active\s*\{[^}]*\n\s*border:/);
  });
});

describe('the state dot', () => {
  it.each(['done', 'warning', 'error'] as const)('draws %s as a dot, silently', (state) => {
    const { container } = render(<StateDot state={state} />);
    const dot = container.querySelector('.statedot');
    expect(dot).toHaveAttribute('data-state', state);
    // Hidden from the accessibility tree on purpose: every caller pairs it
    // with the word, and announcing both says the state twice.
    expect(dot).toHaveAttribute('aria-hidden', 'true');
  });

  it('draws running as a chase rather than a colour', () => {
    // "Running" is a claim about right now, and a still dot cannot say
    // whether it is still true.
    const { container } = render(<StateDot state="ongoing" />);
    expect(container.querySelector('.statedot')).toBeNull();
    expect(container.querySelectorAll('.statedot__cell')).toHaveLength(8);
  });

  it('starts the chase mid-cycle so it is already running at mount', () => {
    // Every cell carries a negative delay. Without them the ring assembles
    // itself over the first second, which reads as loading rather than as
    // running.
    const { container } = render(<StateDot state="ongoing" />);
    const delays = [...container.querySelectorAll<HTMLElement>('.statedot__cell')].map(
      (cell) => cell.style.animationDelay,
    );
    expect(delays).toEqual([
      '-1000ms',
      '-875ms',
      '-750ms',
      '-625ms',
      '-500ms',
      '-375ms',
      '-250ms',
      '-125ms',
    ]);
  });

  it('takes the size it is given', () => {
    const { container } = render(<StateDot state="done" size={16} />);
    const dot = container.querySelector<HTMLElement>('.statedot');
    expect(dot?.style.width).toBe('16px');
    expect(dot?.style.height).toBe('16px');
  });

  it('stops moving for somebody who asked it to', () => {
    // A reduced-motion request is not a preference about this component; it
    // is the answer for anything that would otherwise animate indefinitely.
    const css = readFileSync(join(SRC, 'ui', 'StateDot.css'), 'utf8');
    expect(css).toMatch(/prefers-reduced-motion: reduce[\s\S]*animation: none/);
  });
});
