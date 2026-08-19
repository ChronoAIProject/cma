import { readFileSync, readdirSync } from 'node:fs';
import { join, relative, resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Button } from './Button';
import { Select } from './Select';

/**
 * Controls — docs/Web_Style.md §7.
 *
 * The five control heights and no others, `box-sizing: border-box`
 * everywhere, the lift state contract, and one dropdown pattern. jsdom does
 * not lay out, so the height rule is checked where it is actually decided:
 * in the stylesheets, which is also where a sixth height would be introduced.
 */

const SRC = resolve(process.cwd(), 'src');

/** Every height §7 permits a control to take, plus the chip. */
const PERMITTED_HEIGHTS = new Set([
  'var(--control-h)',
  'var(--control-h-sm)',
  'var(--control-h-md)',
  'var(--control-h-lg)',
  'var(--control-h-xl)',
  'var(--chip-h)',
  'var(--hit)',
  'var(--band-head)',
  'var(--band-foot)',
  'var(--bar-height)',
  'var(--canvas-node-h)',
  '100%',
  'auto',
  '1em',
  '1',
  // A textarea grows; its floor is stated in control heights rather than in
  // pixels, which is the property this check exists to hold.
  'calc(var(--control-h) * 2)',
  // §18.3's own number: the approval's answers sit two above the field step.
  'calc(var(--control-h-md) + 2px)',
]);

function stylesheets(dir: string): string[] {
  const found: string[] = [];
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const path = join(dir, entry.name);
    if (entry.isDirectory()) {
      found.push(...stylesheets(path));
    } else if (entry.name.endsWith('.css')) {
      found.push(path);
    }
  }
  return found;
}

describe('the control heights', () => {
  it('takes a height from §7 rather than a hand-set pixel value', () => {
    // A control that sets its own pixel height is how two controls in one row
    // end up a pixel out of line. Small decorative boxes — a status dot, an
    // avatar — are not controls and are allowed their own geometry, so the
    // check is on the token vocabulary rather than on every declaration.
    const offences: string[] = [];
    for (const path of stylesheets(SRC)) {
      const css = readFileSync(path, 'utf8');
      for (const [index, line] of css.split('\n').entries()) {
        const match = /^\s*(?:min-)?height:\s*(.+);/.exec(line);
        const value = match?.[1];
        if (value === undefined || PERMITTED_HEIGHTS.has(value)) {
          continue;
        }
        // A pixel height on something that is not a control is fine; a pixel
        // height inside a rule naming a button, field, select or chip is not.
        const rule = css.slice(0, css.indexOf(line)).split('}').pop() ?? '';
        if (/\.(button|field|select|menu__item|chip|segment)/.test(rule)) {
          offences.push(`${relative(SRC, path)}:${String(index + 1)} ${line.trim()}`);
        }
      }
    }
    expect(offences).toEqual([]);
  });

  it('makes every element border-box', () => {
    // §7: a quiet button carries a 1px border and an accent button does not,
    // so under content-box the two render 2px apart and sit a pixel out of
    // line in the same row.
    const base = readFileSync(join(SRC, 'styles', 'base.css'), 'utf8');
    expect(base).toMatch(/\*,\s*\*::before,\s*\*::after\s*\{\s*box-sizing: border-box;/);
  });
});

describe('the button', () => {
  it('takes the primary fill when the surface spends it', () => {
    const { container } = render(<Button tone="accent">Start</Button>);
    expect(container.firstElementChild).toHaveClass('button', 'button--primary');
  });

  it('takes the disabled treatment, and the reason lives beside it', () => {
    // §7: disabled, never removed — and the reason is a sibling
    // .control-reason drawn once per row, never stacked into the key, which
    // made every disabled control taller than its enabled neighbour.
    render(
      <div>
        <Button disabled>End session</Button>
        <span className="control-reason">This session is not running</span>
      </div>,
    );
    const button = screen.getByRole('button', { name: /end session/i });
    expect(button).toBeDisabled();
    expect(button.textContent).not.toContain('This session is not running');

    const components = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    // §3: disabled takes the inset fill with an --ink-3 label.
    expect(components).toMatch(/\.button:disabled,[\s\S]*?background: var\(--inset\)/);
    // The reason class is part of the shared vocabulary, at note size in ink-3.
    expect(components).toMatch(/\.control-reason\s*\{[^}]*font-size: var\(--text-note\)/);
  });

  it('is quiet unless something is spent on it', () => {
    // §7: one primary per surface, spent on the action it exists for.
    const { container } = render(<Button>Cancel</Button>);
    expect(container.firstElementChild).toHaveClass('button');
    expect(container.firstElementChild).not.toHaveClass('button--primary');
  });
});

describe('the dropdown', () => {
  const OPTIONS = [
    { value: 'a', label: 'Codex' },
    { value: 'b', label: 'Claude Code' },
    { value: 'c', label: 'DeepSeek Harness', unavailable: 'no terminal plane' },
  ] as const;

  function renderSelect(onChange = vi.fn()) {
    render(<Select label="Agent" value="a" options={OPTIONS} onChange={onChange} />);
    return onChange;
  }

  it('does not run anything when it opens', async () => {
    // §7: choosing a value is a select and choosing an action is a menu. They
    // look alike and behave differently.
    const user = userEvent.setup();
    const onChange = renderSelect();
    await user.click(screen.getByRole('combobox', { name: 'Agent' }));
    expect(screen.getByRole('listbox')).toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('is operable from the keyboard alone', async () => {
    // §10: every view is fully keyboard operable. A native <select> gave this
    // away for free and the pattern §7 asks for cannot, so it is implemented.
    const user = userEvent.setup();
    const onChange = renderSelect();
    const trigger = screen.getByRole('combobox', { name: 'Agent' });
    trigger.focus();

    await user.keyboard('{ArrowDown}');
    expect(trigger).toHaveAttribute('aria-expanded', 'true');
    await user.keyboard('{ArrowDown}{Enter}');

    expect(onChange).toHaveBeenCalledWith('b');
  });

  it('closes on Escape without choosing', async () => {
    const user = userEvent.setup();
    const onChange = renderSelect();
    const trigger = screen.getByRole('combobox', { name: 'Agent' });
    trigger.focus();

    await user.keyboard('{ArrowDown}{Escape}');

    expect(trigger).toHaveAttribute('aria-expanded', 'false');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('disables an unavailable option rather than removing it, and steps past it', async () => {
    const user = userEvent.setup();
    const onChange = renderSelect();
    const trigger = screen.getByRole('combobox', { name: 'Agent' });
    trigger.focus();

    await user.keyboard('{ArrowDown}');
    const unavailable = screen.getByRole('option', { name: /DeepSeek Harness/ });
    expect(unavailable).toHaveAttribute('aria-disabled', 'true');
    expect(unavailable.textContent).toContain('no terminal plane');

    // End would land on the last option; it lands on the last CHOOSABLE one.
    await user.keyboard('{End}{Enter}');
    expect(onChange).toHaveBeenCalledWith('b');
  });

  it('marks the chosen option by weight and by a tick', () => {
    // §7 asks for both, because a mark carried by one channel alone is not a
    // mark to everyone (§10).
    const components = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(components).toMatch(/\.menu__item\[aria-selected='true'\][\s\S]*?font-weight: 600/);
    expect(components).toMatch(/\.menu__tick::before\s*\{\s*content: '✓'/);
  });

  it('holds the 200px minimum when it filters, and flips rather than rotates', () => {
    const components = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(components).toMatch(/\.select--filter\s*\{\s*min-width: 200px/);
    // Content swap, not a transform: at this size a rotation reads as a wobble.
    expect(components).toMatch(/\.select\[aria-expanded='true'\] \.select__caret::before/);
    expect(components).not.toMatch(/\.select__caret[\s\S]{0,120}transform:/);
  });
});
