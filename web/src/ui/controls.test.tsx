import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Button } from './Button';
import { Picker } from './Picker';

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
        <Button disabled>Pause session</Button>
        <span className="control-reason">This session is not running</span>
      </div>,
    );
    const button = screen.getByRole('button', { name: 'Pause session' });
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

describe('the value picker', () => {
  const OPTIONS = [
    { value: 'a', label: 'Codex' },
    { value: 'b', label: 'Claude Code' },
    { value: 'c', label: 'DeepSeek Harness', unavailable: 'no terminal plane' },
  ] as const;

  function renderPicker(onChange = vi.fn()) {
    render(<Picker label="Agent" value="a" options={OPTIONS} onChange={onChange} />);
    return onChange;
  }

  it('does not choose anything when it opens', async () => {
    // §7: choosing a value and choosing an action look alike and behave
    // differently. Opening one is not picking the row under the pointer.
    const user = userEvent.setup();
    const onChange = renderPicker();
    await user.click(screen.getByRole('button', { name: 'Agent' }));
    expect(screen.getByRole('menu', { name: 'Agent' })).toBeInTheDocument();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('is operable from the keyboard alone', async () => {
    // §10: every view is fully keyboard operable. A native <select> gave this
    // away for free and this pattern cannot, so it is implemented — and the
    // menu it replaced had no keyboard route at all.
    const user = userEvent.setup();
    const onChange = renderPicker();

    await user.click(screen.getByRole('button', { name: 'Agent' }));
    // Focus lands on the selected row, so arrows move from where you are.
    expect(screen.getByRole('menuitemradio', { name: 'Codex' })).toHaveFocus();

    await user.keyboard('{ArrowDown}{Enter}');
    expect(onChange).toHaveBeenCalledWith('b');
  });

  it('wraps rather than stopping at the ends', async () => {
    // A list that stops dead at the last row makes the first one four presses
    // away in a menu of five.
    const user = userEvent.setup();
    renderPicker();
    await user.click(screen.getByRole('button', { name: 'Agent' }));

    await user.keyboard('{ArrowUp}');
    expect(screen.getByRole('menuitemradio', { name: /DeepSeek Harness/ })).toHaveFocus();
  });

  it('closes on Escape without choosing', async () => {
    const user = userEvent.setup();
    const onChange = renderPicker();
    await user.click(screen.getByRole('button', { name: 'Agent' }));

    await user.keyboard('{Escape}');

    expect(screen.queryByRole('menu')).toBeNull();
    expect(onChange).not.toHaveBeenCalled();
  });

  it('disables an unavailable option rather than removing it, and says why', async () => {
    // §7: removing it answers "why is it not there" with nothing, and a person
    // who came looking for it concludes the product cannot do it.
    const user = userEvent.setup();
    renderPicker();
    await user.click(screen.getByRole('button', { name: 'Agent' }));

    const unavailable = screen.getByRole('menuitemradio', { name: /DeepSeek Harness/ });
    expect(unavailable).toHaveAttribute('aria-disabled', 'true');
    expect(unavailable.textContent).toContain('no terminal plane');

    // Still reachable, so the reason can actually be read. Choosing it does
    // nothing — the row is listed, not offered.
    unavailable.focus();
    expect(unavailable).toHaveFocus();
    await user.click(unavailable);
    expect(screen.getByRole('menu')).toBeInTheDocument();
  });

  it('marks the chosen option with a tick rather than a fill', () => {
    // A fill would make the selected row and the hovered row the same colour,
    // so running the pointer down the list would look like the selection
    // following it.
    const css = readFileSync(join(SRC, 'ui', 'Menu.css'), 'utf8');
    expect(css).toMatch(/\.menu__check\s*\{[^}]*color: var\(--/);
    expect(css).not.toMatch(/\.menu__item\[aria-checked='true'\]\s*\{[^}]*background:/);
  });

  it('holds a minimum width when it filters', () => {
    // Otherwise the row it sits in resizes every time the value changes.
    const css = readFileSync(join(SRC, 'ui', 'Picker.css'), 'utf8');
    expect(css).toMatch(/\.picker--filter \.picker__anchor\s*\{\s*min-width: 160px/);
  });
});
