import { fireEvent, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import type { AgentUsage } from '../api/types';
import { ContextRing, contextFraction } from './ContextRing';

const usage: AgentUsage = {
  agentId: 'agt_1',
  throughSeq: 4,
  inputTokens: 1200,
  outputTokens: 300,
  contextUsed: 54210,
  contextWindow: 200000,
  currentTurn: { turnSequence: 3, inputTokens: 900, outputTokens: 40 },
};

const ring = () => screen.getByRole('button', { name: 'Context usage' });

it('draws the arc to scale and lists context, totals and the current turn', () => {
  const { container } = render(<ContextRing usage={usage} />);
  const arc = container.querySelector('.context-ring__arc')!;
  const circumference = 2 * Math.PI * 8;
  const [drawn] = arc.getAttribute('stroke-dasharray')!.split(' ').map(Number);
  expect(drawn).toBeCloseTo(circumference * 0.27105, 3);
  expect(ring()).toHaveAccessibleDescription(
    'Context54,210 / 200,000 (27%)Total tokens1,5001,200 input · 300 outputThis turn900 input · 40 output',
  );
});

it('says not reported instead of drawing zero when usage or its ceiling is missing', () => {
  const { container, rerender } = render(<ContextRing />);
  expect(container.querySelector('.context-ring__arc')).toBeNull();
  expect(ring()).toHaveAccessibleDescription('Contextnot reportedTotal tokensnot reported');
  rerender(<ContextRing usage={{ ...usage, contextWindow: undefined, currentTurn: undefined }} />);
  expect(container.querySelector('.context-ring__arc')).toBeNull();
  expect(ring()).toHaveAccessibleDescription(/^Context54,210Total tokens/);
  expect(contextFraction({ ...usage, contextWindow: 0 })).toBeUndefined();
  expect(contextFraction({ ...usage, contextUsed: 300000 })).toBe(1);
});

it('shows the tooltip on hover and on keyboard focus, and Escape hides it without moving focus', async () => {
  const user = userEvent.setup();
  render(<ContextRing usage={usage} />);
  const tooltip = screen.getByRole('tooltip', { hidden: true });
  expect(tooltip).not.toBeVisible();
  await user.hover(ring());
  expect(tooltip).toBeVisible();
  await user.unhover(ring());
  expect(tooltip).not.toBeVisible();
  await user.tab();
  expect(ring()).toHaveFocus();
  expect(tooltip).toBeVisible();
  expect(ring()).toHaveAttribute('aria-expanded', 'true');
  await user.keyboard('{Escape}');
  expect(tooltip).not.toBeVisible();
  expect(ring()).toHaveFocus();
  fireEvent.blur(ring());
  fireEvent.focus(ring());
  expect(tooltip).toBeVisible();
});

it('toggles the tooltip with a press for touch, and closes it when focus leaves', async () => {
  const user = userEvent.setup();
  render(
    <>
      <ContextRing usage={usage} />
      <button type="button">Elsewhere</button>
    </>,
  );
  const tooltip = screen.getByRole('tooltip', { hidden: true });
  fireEvent.click(ring());
  expect(tooltip).toBeVisible();
  fireEvent.click(ring());
  expect(tooltip).not.toBeVisible();
  fireEvent.click(ring());
  await user.click(screen.getByRole('button', { name: 'Elsewhere' }));
  expect(tooltip).not.toBeVisible();
});
