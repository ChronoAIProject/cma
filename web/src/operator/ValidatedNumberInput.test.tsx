import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { ValidatedNumberInput } from './ValidatedNumberInput';

it('validates initial values and changing constraints using native validity', () => {
  const field = (value: number, min: number) => (
    <ValidatedNumberInput aria-label="Count" value={value} min={min} readOnly />
  );
  // Read-only controls are excluded from native constraint validation.
  const view = render(field(-1, 0));
  expect(screen.getByLabelText('Count')).toHaveAttribute('aria-invalid', 'false');
  view.rerender(<ValidatedNumberInput aria-label="Count" value={-1} min={0} onChange={() => {}} />);
  const input = screen.getByLabelText('Count') as HTMLInputElement;
  expect(input).toHaveAttribute('aria-invalid', 'true');
  expect(input).toHaveAccessibleDescription(input.validationMessage);
  view.rerender(
    <ValidatedNumberInput aria-label="Count" value={-1} min={-2} onChange={() => {}} />,
  );
  expect(input).toHaveAttribute('aria-invalid', 'false');
});

it('preserves explicit server errors and permits fractional steps', () => {
  const view = render(
    <ValidatedNumberInput
      aria-label="Amount"
      value={1.5}
      step="any"
      error="Server refused"
      onChange={() => {}}
    />,
  );
  const input = screen.getByLabelText('Amount');
  expect(input).toHaveAttribute('aria-invalid', 'true');
  expect(input).toHaveAccessibleDescription('Server refused');
  view.rerender(
    <ValidatedNumberInput aria-label="Amount" value={1.5} step="any" onChange={() => {}} />,
  );
  expect(input).toHaveAttribute('aria-invalid', 'false');
  expect(input).not.toHaveAttribute('aria-describedby');
});
