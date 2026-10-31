import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ConversationEmpty } from './ConversationEmpty';

vi.mock('../character/stageBrowser', () => ({ characterStage: { attach: () => undefined } }));

it('greets a ready Agent with its idle character above the prompt', () => {
  const { container } = render(<ConversationEmpty message="Start the conversation." />);
  expect(screen.getByText('Start the conversation.')).toBeInTheDocument();
  const character = container.querySelector('.character-avatar');
  expect(character).toHaveAttribute('data-pose', 'idle');
  expect(character).toHaveAttribute('data-motion', 'live');
  expect(character).toHaveStyle({ width: '120px', height: '120px' });
});

it.each([
  ['paused', 'paused', 'brand'],
  ['failed', 'failed', 'error'],
] as const)('holds a %s Agent still', (status, pose, tone) => {
  const { container } = render(<ConversationEmpty message="No messages yet." status={status} />);
  const character = container.querySelector('.character-avatar');
  expect(character).toHaveAttribute('data-pose', pose);
  expect(character).toHaveAttribute('data-tone', tone);
  expect(character).toHaveAttribute('data-motion', 'still');
});
