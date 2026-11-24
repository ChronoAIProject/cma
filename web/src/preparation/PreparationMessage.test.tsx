import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { PreparationMessage } from './PreparationMessage';

it('keeps the exact multiline request as selectable plain text', () => {
  const text = '  原文\n<img src=x> **literal**\nFinal line  ';
  render(<PreparationMessage text={text} />);
  const original = screen.getByRole('region', { name: 'Original request' });
  expect(original.querySelector('p')!.textContent).toBe(text);
  expect(original.querySelector('img')).toBeNull();
  expect(screen.queryByRole('button')).toBeNull();
});

it('provides an explicit expand/collapse control without replacing the long request bytes', () => {
  const text = 'Long request 中文\n'.repeat(200);
  render(<PreparationMessage text={text} />);
  const button = screen.getByRole('button', { name: 'Show more' });
  const content = document.getElementById(button.getAttribute('aria-controls')!)!;
  expect(content.textContent).toBe(text);
  expect(button).toHaveAttribute('aria-expanded', 'false');
  fireEvent.click(button);
  expect(screen.getByRole('button', { name: 'Show less' })).toHaveAttribute(
    'aria-expanded',
    'true',
  );
  expect(content).not.toHaveAttribute('data-collapsed');
  expect(content.textContent).toBe(text);
  fireEvent.click(button);
  expect(content).toHaveAttribute('data-collapsed', 'true');
});
