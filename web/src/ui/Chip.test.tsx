import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { Chip } from './Chip';

/**
 * The pill's job is to say the state in words. The dot and its colour are
 * reinforcement, so what is pinned here is that the word is always present
 * and that the tone is carried alongside it rather than instead of it.
 */

describe('Chip', () => {
  it('always draws the state word beside the dot', () => {
    render(<Chip tone="fail">denied</Chip>);
    const pill = screen.getByText('denied');
    expect(pill).toBeInTheDocument();
    // §2: a semantic colour never carries meaning alone — the dot is
    // decorative and hidden from assistive technology, the word is the state.
    expect(pill.querySelector('.status__dot')).toHaveAttribute('aria-hidden', 'true');
  });

  it('carries the tone as well as the word', () => {
    render(<Chip tone="live">ready</Chip>);
    expect(screen.getByText('ready')).toHaveClass('status--live');
  });

  it('is neutral unless told otherwise', () => {
    // An unfamiliar state must not be dressed as a healthy one.
    render(<Chip>hibernating</Chip>);
    expect(screen.getByText('hibernating')).toHaveClass('status--ended');
  });

  it('takes leading space only when it sits in a line of prose', () => {
    const { rerender } = render(<Chip>required</Chip>);
    expect(screen.getByText('required')).not.toHaveClass('chip--inline');
    rerender(<Chip inline>required</Chip>);
    expect(screen.getByText('required')).toHaveClass('chip--inline');
  });
});
