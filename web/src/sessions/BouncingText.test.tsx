import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { BouncingText } from './BouncingText';

/**
 * The animation is decoration. What matters is that the sentence survives being
 * split into characters — for assistive technology, and for anyone who has
 * asked for less motion.
 */

describe('BouncingText', () => {
  it('exposes the sentence once rather than as loose characters', () => {
    // Split into spans, a screen reader would otherwise spell it out.
    render(<BouncingText text="Starting a sandbox" />);
    expect(screen.getByText('Starting a sandbox')).toBeInTheDocument();
  });

  it('renders one animated element per character', () => {
    const { container } = render(<BouncingText text="abc" />);
    expect(container.querySelectorAll('.bounce__char')).toHaveLength(3);
  });

  it('staggers each character after the one before it', () => {
    // The wave is the point: without an increasing delay every character
    // moves together, which reads as a flash rather than a wave.
    const { container } = render(<BouncingText text="abc" stagger={0.1} />);
    const delays = Array.from(container.querySelectorAll<HTMLElement>('.bounce__char')).map(
      (el) => el.style.animationDelay,
    );
    // The browser normalises the value, so compare the numbers rather than
    // the exact string we happened to write.
    expect(delays.map((d) => Number.parseFloat(d))).toEqual([0, 0.1, 0.2]);
  });

  it('keeps spaces so words stay apart', () => {
    const { container } = render(<BouncingText text="a b" />);
    expect(container.querySelectorAll('.bounce__char')).toHaveLength(3);
    expect(container.textContent).toContain('a b');
  });

  it('handles an empty string without producing anything odd', () => {
    const { container } = render(<BouncingText text="" />);
    expect(container.querySelectorAll('.bounce__char')).toHaveLength(0);
  });

  it('hides the per-character copy from assistive technology', () => {
    // Otherwise the sentence is announced twice.
    const { container } = render(<BouncingText text="hi" />);
    const decorative = container.querySelector('[aria-hidden="true"]');
    expect(decorative).not.toBeNull();
    expect(decorative?.querySelectorAll('.bounce__char')).toHaveLength(2);
  });
});
