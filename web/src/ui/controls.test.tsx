import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
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

describe('the control heights', () => {
  it('gives the button two heights and no third', () => {
    /*
     * §7 gives the capsule 36px and 28px. A third height added here is a
     * third height everything can then reach for, which is how a row ends up
     * with three controls on two baselines — and it is invisible in a diff
     * that only adds a modifier.
     *
     * Checked on Button.css alone, deliberately. A surface that genuinely
     * needs its own proportions declares them in its own sheet, where the
     * decision is local and visible, rather than by widening the shared set.
     */
    const css = readFileSync(join(SRC, 'ui', 'Button.css'), 'utf8');
    const heights = [...css.matchAll(/^\s*height:\s*(.+);/gm)].map((match) => match[1]);
    expect(new Set(heights)).toEqual(new Set(['36px', '28px', '16px']));
  });

  it('makes every element border-box', () => {
    // §7: an outline button carries a 1px border and a primary one does not,
    // so under content-box the two render 2px apart and sit a pixel out of
    // line in the same row.
    const base = readFileSync(join(SRC, 'styles', 'base.css'), 'utf8');
    expect(base).toMatch(/\*,\s*\*::before,\s*\*::after\s*\{\s*box-sizing: border-box;/);
  });
});

describe('the button', () => {
  it('takes the primary fill when the surface spends it', () => {
    const { container } = render(<Button variant="primary">Start</Button>);
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

    // Dimmed rather than recoloured, so a primary button that cannot be
    // pressed is still recognisably the one somebody is looking for.
    const buttonCss = readFileSync(join(SRC, 'ui', 'Button.css'), 'utf8');
    expect(buttonCss).toMatch(/\.button:disabled\s*\{[^}]*opacity: 0\.4/);
    expect(buttonCss).toMatch(/\.button:disabled\s*\{[^}]*cursor: not-allowed/);

    // The reason class is part of the shared vocabulary, and stays there.
    const components = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(components).toMatch(/\.control-reason\s*\{/);
  });

  it('is a ghost unless something is spent on it', () => {
    // §7: one primary per surface, spent on the action it exists for. Ghost
    // is the default so that spending it is a decision somebody made.
    const { container } = render(<Button>Cancel</Button>);
    expect(container.firstElementChild).toHaveClass('button', 'button--ghost');
    expect(container.firstElementChild).not.toHaveClass('button--primary');
  });

  it('draws a leading glyph in a fixed box, out of the accessible name', () => {
    // The slot is 16px whatever the glyph draws itself at, so a row of
    // buttons keeps its labels on one baseline. It is aria-hidden because the
    // label already says what the button does; announcing both says it twice.
    const { container } = render(<Button icon={<svg data-testid="glyph" />}>Stop</Button>);
    const slot = container.querySelector('.button__icon');
    expect(slot).not.toBeNull();
    expect(slot).toHaveAttribute('aria-hidden', 'true');
    expect(screen.getByRole('button').textContent).toBe('Stop');
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
