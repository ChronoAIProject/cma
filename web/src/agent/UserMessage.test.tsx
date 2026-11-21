import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { UserMessage } from './UserMessage';
import { attachmentMarker } from './attachments';

it('shows the user request and compact attachment records while preserving the exact path in the title', () => {
  render(
    <UserMessage
      content={`Review my notes${attachmentMarker}${JSON.stringify([
        {
          name: 'notes.txt',
          path: '/home/attachments/agt_1/att_1/notes.txt',
          media_type: 'text/plain',
          size_bytes: 16,
        },
      ])}`}
    />,
  );
  expect(screen.getByText('Review my notes')).toBeInTheDocument();
  expect(screen.getByRole('list', { name: 'Attached files' })).toHaveTextContent('notes.txt');
  expect(screen.getByRole('listitem')).toHaveAttribute(
    'title',
    '/home/attachments/agt_1/att_1/notes.txt',
  );
});

it.each([
  'ordinary message',
  `Literal example${attachmentMarker}{not json}`,
  `Bad shape${attachmentMarker}[null]`,
])('preserves message text when it does not contain a valid attachment manifest', (content) => {
  const { container } = render(<UserMessage content={content} />);
  expect(container.querySelector('p')?.textContent).toBe(content);
  expect(screen.queryByRole('list')).not.toBeInTheDocument();
});
