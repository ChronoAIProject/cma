import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { taskStateMeta } from './taskStatus';

it('gives Ready to start and Completed distinct static dot tones', () => {
  render(
    <>
      {taskStateMeta('not_started')}
      {taskStateMeta('completed')}
      {taskStateMeta('working')}
    </>,
  );
  expect(screen.getByText('Ready to start')).toHaveAttribute('data-tone', 'ready');
  expect(screen.getByText('Completed')).toHaveAttribute('data-tone', 'completed');
  expect(screen.getByText('Ready to start').querySelector('[aria-hidden]')).toHaveAttribute(
    'data-tone',
    'ready',
  );
  expect(screen.getByText('Completed').querySelector('[aria-hidden]')).toHaveAttribute(
    'data-tone',
    'completed',
  );
  expect(screen.getByText('Working')).toHaveAttribute('data-tone', 'active');
});
it('keeps stale task observations muted without a status dot', () => {
  render(
    <>
      {taskStateMeta('not_started', true)}
      {taskStateMeta('completed', true)}
    </>,
  );
  for (const label of ['Ready to start', 'Completed']) {
    expect(screen.getByText(label)).toHaveAttribute('data-tone', 'muted');
    expect(screen.getByText(label).querySelector('[aria-hidden]')).toBeNull();
  }
});
