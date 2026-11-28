import { readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Button } from './Button';
import { Picker } from './Picker';

/**
 * Controls — the shared style contracts
 *
 * The two shared control heights and no others, `box-sizing: border-box`
 * everywhere, the lift state contract, and one dropdown pattern. jsdom does
 * not lay out, so the height rule is checked where it is actually decided:
 * in the stylesheets, which is also where a sixth height would be introduced.
 */

const SRC = resolve(process.cwd(), 'src');

describe('the control heights', () => {
  it('gives the button two heights and no third', () => {
    /*
     * §12 gives the control 40px and 32px. A third height added here is a
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
    expect(new Set(heights)).toEqual(new Set(['40px', '32px', '16px']));
    expect(css).toMatch(/\.button\s*\{[^}]*border-radius: 10px/);
    expect(css).toMatch(/\.button--sm\s*\{[^}]*border-radius: 6px/);
    expect(css).toMatch(/\.button--md\.button--icon-only\s*\{[^}]*width: 40px/);
    expect(css).toMatch(/\.button--sm\.button--icon-only\s*\{[^}]*width: 32px/);
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

  it('renders a danger action from danger tokens and never the primary fill', () => {
    const { container } = render(<Button variant="danger">Remove</Button>);
    expect(container.firstElementChild).toHaveClass('button', 'button--danger');

    const css = readFileSync(join(SRC, 'ui', 'Button.css'), 'utf8');
    const dangerRules = [...css.matchAll(/\.button--danger[^{]*\{[^}]*\}/g)]
      .map((match) => match[0])
      .join('\n');
    expect(dangerRules).toContain('var(--dsw-alias-state-error-primary)');
    expect(dangerRules).toContain('var(--dsw-alias-state-error-secondary)');
    expect(dangerRules).toContain('var(--dsw-alias-interactive-bg-hover-danger)');
    expect(dangerRules).not.toContain('button-primary');
  });

  it('activates a danger action from the keyboard and leaves it inert when disabled', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    const { rerender } = render(
      <Button variant="danger" onClick={onClick}>
        Remove
      </Button>,
    );

    const button = screen.getByRole('button', { name: 'Remove' });
    button.focus();
    await user.keyboard('{Enter}');
    expect(onClick).toHaveBeenCalledOnce();

    rerender(
      <Button variant="danger" disabled onClick={onClick}>
        Remove
      </Button>,
    );
    await user.click(screen.getByRole('button', { name: 'Remove' }));
    expect(onClick).toHaveBeenCalledOnce();
  });

  it('takes interaction values from the named component tokens', () => {
    const css = readFileSync(join(SRC, 'ui', 'Button.css'), 'utf8');
    expect(css).toMatch(
      /\.button--primary:hover:not\(:disabled, \[aria-disabled='true'\]\)\s*\{[^}]*box-shadow: var\(--dsw-shadow-accent-hover\)/,
    );
    expect(css).toMatch(
      /\.button--primary:active:not\(:disabled, \[aria-disabled='true'\]\)\s*\{[^}]*var\(--dsw-alias-button-primary-active\)/,
    );
    expect(css).toMatch(/\.button--outline\s*\{[^}]*var\(--dsw-alias-border-l3\)/);
    expect(css).toMatch(
      /\.button--outline:hover:not\(:disabled, \[aria-disabled='true'\]\)\s*\{[^}]*var\(--dsw-alias-interactive-bg-hover\)[^}]*var\(--dsw-alias-border-l4\)/,
    );
  });

  it('takes the disabled treatment, and the reason lives beside it', () => {
    // §12: disabled, never removed — and the reason is a sibling
    // .control-reason drawn once per row, never stacked into the key, which
    // made every disabled control taller than its enabled neighbour.
    render(
      <div>
        <Button disabled>Pause resource</Button>
        <span className="control-reason">This resource is not running</span>
      </div>,
    );
    const button = screen.getByRole('button', { name: 'Pause resource' });
    expect(button).toBeDisabled();
    expect(button.textContent).not.toContain('This resource is not running');

    // Dimmed rather than recoloured, so a primary button that cannot be
    // pressed is still recognisably the one somebody is looking for.
    const buttonCss = readFileSync(join(SRC, 'ui', 'Button.css'), 'utf8');
    expect(buttonCss).toMatch(
      /\.button\[aria-disabled='true'\],\s*\.button:disabled\s*\{[^}]*opacity: 0\.4/,
    );
    expect(buttonCss).toMatch(/\.button:disabled\s*\{[^}]*cursor: not-allowed/);

    // The reason class is part of the shared vocabulary, and stays there.
    const components = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(components).toMatch(/\.control-reason\s*\{/);
  });

  it('is a ghost unless something is spent on it', () => {
    // §12: one primary per surface, spent on the action it exists for. Ghost
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

  it('only applies square geometry when the icon has no label', () => {
    const { rerender } = render(<Button icon={<svg />}>Check access</Button>);
    const labelled = screen.getByRole('button', { name: 'Check access' });
    expect(labelled).not.toHaveClass('button--icon-only');

    rerender(<Button icon={<svg aria-label="Refresh" />} />);
    const iconOnly = screen.getByRole('button');
    expect(iconOnly).toHaveClass('button--icon-only', 'button--md');

    const css = readFileSync(join(SRC, 'ui', 'Button.css'), 'utf8');
    expect(css).not.toContain(':has(> .button__icon:only-child)');
  });
});

describe('the shared component vocabulary', () => {
  it('uses no migration-shim property except the shared focus ring', () => {
    const tokens = readFileSync(join(SRC, 'styles', 'tokens.css'), 'utf8');
    const migration = tokens.slice(tokens.indexOf('Migration shim'));
    const shimBody = /body\s*\{([\s\S]*?)\n\}/.exec(migration)?.[1] ?? '';
    const shimNames = new Set(
      [...shimBody.matchAll(/^\s*(--[a-z0-9-]+):/gm)].map((match) => match[1]),
    );
    shimNames.delete('--focus-ring');

    const components = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    const references = [...components.matchAll(/var\((--[a-z0-9-]+)/g)].map((match) => match[1]);
    expect(references.filter((name) => shimNames.has(name))).toEqual([]);
  });

  it('keeps the four cross-sheet transition owners in one shared rule', () => {
    const css = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(css).toMatch(/\.button,\s*\.pill,\s*\.row-item,\s*\.input\s*\{\s*transition:/);
    expect(css).toMatch(
      /prefers-reduced-motion: reduce[\s\S]*\.button,\s*\.pill,\s*\.row-item,\s*\.input\s*\{\s*transition: none/,
    );
  });

  it('settles a pressed button to 96% and keeps it still under reduced motion', () => {
    const css = readFileSync(join(SRC, 'ui', 'Button.css'), 'utf8');
    expect(css).toMatch(
      /\.button:active:not\(:disabled, \[aria-disabled='true'\]\) \{\s*scale: 0\.96;/,
    );
    expect(css).toMatch(
      /prefers-reduced-motion: reduce[\s\S]*\.button:active:not\(:disabled, \[aria-disabled='true'\]\) \{\s*scale: none;/,
    );
    const shared = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(shared).toMatch(/scale var\(--ds-transition-duration-fast\)/);
  });

  it('pins the field, failure, progress, and skeleton idioms to their aliases', () => {
    const css = readFileSync(join(SRC, 'styles', 'components.css'), 'utf8');
    expect(css).toMatch(
      /\.field__label\s*\{[^}]*color: var\(--dsw-alias-label-caption\)[^}]*font: var\(--dsw-font-xxs-strong-12\)/,
    );
    expect(css).not.toMatch(/\.field__label\s*\{[^}]*text-transform: uppercase/);
    expect(css).toMatch(
      /\.notice--fail\s*\{[^}]*background: var\(--dsw-alias-interactive-bg-hover-danger\)[^}]*border-left: 2px solid var\(--dsw-alias-state-error-primary\)/,
    );
    expect(css).toMatch(
      /\.progress__fill\s*\{[^}]*background: var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--dsw-glow-accent\)/,
    );
    expect(css).toMatch(/\.skeleton\s*\{[^}]*background: var\(--dsw-alias-bg-skeleton\)/);
    expect(css).toMatch(
      /\.skeleton::after\s*\{[^}]*animation: skeleton-shimmer 1\.6s var\(--ds-ease-in-out\) infinite/,
    );
    expect(css).toMatch(
      /prefers-reduced-motion: reduce[\s\S]*\.skeleton::after\s*\{[^}]*display: none[^}]*animation: none/,
    );
  });
});

describe('the value picker', () => {
  const OPTIONS = [
    { value: 'a', label: 'First option' },
    { value: 'b', label: 'Second option' },
    { value: 'c', label: 'Unavailable option', unavailable: 'not available' },
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
    expect(screen.getByRole('menuitemradio', { name: 'First option' })).toHaveFocus();

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
    expect(screen.getByRole('menuitemradio', { name: /Unavailable option/ })).toHaveFocus();
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
    // §12: removing it answers "why is it not there" with nothing, and a person
    // who came looking for it concludes the product cannot do it.
    const user = userEvent.setup();
    renderPicker();
    await user.click(screen.getByRole('button', { name: 'Agent' }));

    const unavailable = screen.getByRole('menuitemradio', { name: /Unavailable option/ });
    expect(unavailable).toHaveAttribute('aria-disabled', 'true');
    expect(unavailable.textContent).toContain('not available');

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

  it('uses the shared menu geometry and keeps danger rows in their own tone', () => {
    const css = readFileSync(join(SRC, 'ui', 'Menu.css'), 'utf8');
    expect(css).toMatch(/\.menu__item\s*\{[^}]*min-height: 40px[^}]*border-radius: 7px/);
    expect(css).toMatch(
      /\.menu__item:hover:not\(\[aria-disabled='true'\]\)\s*\{[^}]*color: var\(--dsw-alias-label-primary\)[^}]*background: var\(--dsw-alias-interactive-bg-hover\)/,
    );
    expect(css).toMatch(
      /\.menu__item--danger:hover:not\(\[aria-disabled='true'\]\)[^{]*\{[^}]*color: var\(--dsw-alias-state-error-primary\)[^}]*background: var\(--dsw-alias-interactive-bg-hover-danger\)/,
    );
  });

  it('uses the shared focus ring for the risk acknowledgement', () => {
    const css = readFileSync(join(SRC, 'ui', 'RiskConfirmation.css'), 'utf8');
    expect(css).toMatch(
      /\.risk__acknowledgement input:focus-visible\s*\{[^}]*box-shadow: var\(--focus-ring\)/,
    );
  });

  it('holds a minimum width when it filters', () => {
    // Otherwise the row it sits in resizes every time the value changes.
    const css = readFileSync(join(SRC, 'ui', 'Picker.css'), 'utf8');
    expect(css).toMatch(/\.picker--filter \.picker__anchor\s*\{\s*min-width: 160px/);
  });
});
