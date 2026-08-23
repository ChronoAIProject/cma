import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { Input } from './Input';

/**
 * The text field — `docs/Web_Style.md` §12.2.
 *
 * The shape being pinned is where the edge lives: on the wrapper, not on the
 * input. That is what lets a glyph sit inside the field, and it is why a field
 * with an icon and one without focus identically.
 */

describe('the text field', () => {
  it('passes everything through to the real input', async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    render(<Input aria-label="Name" placeholder="unnamed" onChange={onChange} />);

    const field = screen.getByRole('textbox', { name: 'Name' });
    expect(field).toHaveAttribute('placeholder', 'unnamed');

    await user.type(field, 'ok');
    expect(onChange).toHaveBeenCalled();
  });

  it('draws a glyph inside the field, out of the accessible name', () => {
    // The icon is decoration beside a label that already says what the field
    // is for; announcing it says the same thing twice.
    const { container } = render(<Input aria-label="Search" icon={<svg />} />);
    const glyph = container.querySelector('.input__icon');
    expect(glyph).not.toBeNull();
    expect(glyph).toHaveAttribute('aria-hidden', 'true');
    expect(screen.getByRole('textbox', { name: 'Search' })).toBeInTheDocument();
  });

  it('puts the edge on the wrapper and no outline on the input', () => {
    /*
     * If the border were on the input, a leading glyph would have to sit
     * outside it and the two would focus as separate things. And an outline
     * on the input would draw a second edge just inside the first.
     */
    const css = readFileSync(resolve(process.cwd(), 'src/ui/Input.css'), 'utf8');
    expect(css).toMatch(/\.input\s*\{[^}]*border: 1px solid var\(--/);
    expect(css).toMatch(/\.input\s*\{[^}]*border-radius: 10px/);
    expect(css).toMatch(/\.input:focus-within\s*\{[^}]*border-color: var\(--/);
    expect(css).toMatch(
      /\.input:focus-within\s*\{[^}]*border-color: var\(--dsw-alias-state-business-primary\)[^}]*box-shadow: var\(--dsw-ring-accent-soft\)/,
    );
    expect(css).toMatch(/\.input__control\s*\{[^}]*border: none/);
    expect(css).toMatch(/\.input__control\s*\{[^}]*outline: none/);
  });

  it('states invalidity on the edge that is already there', () => {
    // Same reasoning as focus: the control has one edge, and everything it
    // has to say about itself is said with that edge rather than beside it.
    const css = readFileSync(resolve(process.cwd(), 'src/ui/Input.css'), 'utf8');
    expect(css).toMatch(/aria-invalid='true'[\s\S]*?border-color: var\(--dsw-alias-state-error/);
    expect(css).toMatch(
      /aria-invalid='true'[^}]*background: var\(--dsw-alias-interactive-bg-hover-danger\)/,
    );
  });

  it('is not the composer', () => {
    // A message box grows with what is typed into it. Sharing this control
    // would make every search box growable, which is the actual bug that
    // sharing one would introduce rather than a stylistic preference.
    const css = readFileSync(resolve(process.cwd(), 'src/ui/Input.css'), 'utf8');
    expect(css).toMatch(/\.input\s*\{[^}]*height: 44px/);
    expect(css).not.toMatch(/min-height/);
  });
});
