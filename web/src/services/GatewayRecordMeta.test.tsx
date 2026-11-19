import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { GatewayRecordMeta } from './GatewayRecordMeta';

const subject = '00000000-0000-4000-8000-000000000383';
const createdAt = '2026-11-07T00:30:12.123456789Z';
const shortDate = new Date(createdAt).toLocaleDateString([], { month: 'short', day: 'numeric' });
const owners = [
  undefined,
  subject,
  { subject },
  { subject, display_name: '' },
  { subject, display_name: '  ' },
  { subject, display_name: '  Project team  ' },
];

it.each(owners)('keeps compact metadata concise for owner %j', (owner) => {
  const { container } = render(
    <button>
      Updates
      <GatewayRecordMeta compact createdAt={createdAt} owner={owner} />
    </button>,
  );
  expect(screen.getByRole('button')).toHaveAccessibleName(`Updates, Created ${shortDate}`);
  expect(container.querySelector('time')).toHaveTextContent(shortDate);
  expect(container.querySelector('time')).toHaveAttribute('datetime', createdAt);
  expect(container.querySelector('.sr-only')).toHaveTextContent(`, Created ${shortDate}`);
  expect(screen.getByTitle(`Created ${shortDate}`)).toHaveAttribute('aria-hidden', 'true');
  expect(container.textContent).not.toMatch(/00000000|Project team|By |T00:30/);
  expect(container.querySelector('[aria-label]')).toBeNull();
  expect(container.querySelectorAll('[title]')).toHaveLength(1);
});

it.each(owners)('uses only a nonempty authorized display name in full metadata: %j', (owner) => {
  const { container } = render(<GatewayRecordMeta createdAt={createdAt} owner={owner} />);
  const creator = typeof owner === 'object' && owner?.display_name?.trim();
  expect(screen.getByText(`By ${creator || 'Not reported'}`)).toBeInTheDocument();
  expect(container.textContent).not.toContain(subject);
  expect(container.textContent).not.toContain(createdAt);
  const time = container.querySelector('time')!;
  expect(time).toHaveAttribute('datetime', createdAt);
  expect(time.textContent).toBe(
    new Date(createdAt).toLocaleString([], {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    }),
  );
  for (const titled of container.querySelectorAll('[title]')) {
    expect(titled.getAttribute('title')).not.toContain(subject);
    expect(titled.getAttribute('title')).not.toContain(createdAt);
  }
});

it.each([undefined, '', 'not-a-date'])(
  'reports absent or invalid creation dates without using the current date: %j',
  (date) => {
    const { container, rerender } = render(
      <button>
        Updates
        <GatewayRecordMeta compact createdAt={date} owner={subject} />
      </button>,
    );
    expect(screen.getByRole('button')).toHaveAccessibleName('Updates, Created Not reported');
    expect(screen.getByTitle('Created Not reported')).toHaveTextContent('Not reported');
    expect(container.querySelector('time')).toBeNull();
    rerender(<GatewayRecordMeta createdAt={date} owner={{ subject, display_name: '' }} />);
    expect(screen.getByText('Created Not reported')).toBeInTheDocument();
    expect(screen.getByText('By Not reported')).toBeInTheDocument();
    expect(container.querySelector('time')).toBeNull();
    expect(container.textContent).not.toContain('Invalid Date');
  },
);
