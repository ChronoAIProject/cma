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
 * Two heights and no others, `box-sizing: border-box` everywhere, the
 * letterpress state contract, and one dropdown pattern. jsdom does not lay
 * out, so the height rule is checked where it is actually decided: in the
 * stylesheets, which is also where a fourth height would be introduced.
 */

const SRC = resolve(process.cwd(), 'src');

/** Every height §7 permits a control to take, plus the chip and the segment. */
const PERMITTED_HEIGHTS = new Set([
  'var(--control-h)',
  'var(--control-h-sm)',
  'var(--control-h-block)',
  'var(--control-h-lg)',
  'var(--chip-h)',
  'var(--segment-h)',
  'var(--hit)',
  'var(--band-head)',
  'var(--band-foot)',
  'var(--bar-height)',
  'var(--canvas-node-h)',
  'var(--stacked-min)',
  '100%',
  'auto',
  '1em',
  '1',
  // A textarea grows; its floor is stated in control heights rather than in
  // pixels, which is the property this check exists to hold.
  'calc(var(--control-h) * 2)',
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
  it('carries the impression its tone calls for', () => {
    const { container } = render(<Button tone="accent">Start</Button>);
    expect(container.firstElementChild).toHaveClass('button', 'button--accent', 'press--accent');
  });

  it('carries no impression when disabled, and says why', () => {
    // §7: the impression is what says a thing can be pressed, so a greyed key
    // that still looks pressable reads as broken rather than unavailable. The
    // reason is part of the button, because a tooltip is unreachable by touch.
    render(
      <Button disabled reason="This session is not running">
        End session
      </Button>,
    );
    const button = screen.getByRole('button', { name: /end session/i });
    expect(button).toBeDisabled();
    expect(button).toHaveClass('button--stacked');
    expect(button.textContent).toContain('This session is not running');

    const components = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(components).toMatch(/\.press--quiet:disabled[\s\S]*?box-shadow: var\(--press-none\)/);
  });

  it('is quiet unless something is spent on it', () => {
    // §7: one accent button per view, spent on the action the view exists for.
    const { container } = render(<Button>Cancel</Button>);
    expect(container.firstElementChild).toHaveClass('press--quiet');
    expect(container.firstElementChild).not.toHaveClass('button--accent');
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

  it('marks the chosen option on its leading edge and by a tick', () => {
    // §7 asks for both, because a mark carried by colour alone is not a mark
    // to everyone (§10).
    const components = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(components).toMatch(
      /\.menu__item\[aria-selected='true'\][\s\S]*?border-left-color: var\(--accent\)/,
    );
    expect(components).toMatch(/\.menu__tick::before\s*\{\s*content: '✓'/);
  });

  it('holds the 160px minimum when it filters, and flips rather than rotates', () => {
    const components = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(components).toMatch(/\.select--filter\s*\{\s*min-width: var\(--dropdown-min\)/);
    // Content swap, not a transform: at 10px a rotation reads as a wobble.
    expect(components).toMatch(/\.select\[aria-expanded='true'\] \.select__caret::before/);
    expect(components).not.toMatch(/\.select__caret[\s\S]{0,120}transform:/);
  });
});
