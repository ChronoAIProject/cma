import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { QuotedContextSummary } from './QuotedContextSummary';

it('distinguishes payload text, truncated text and ID-only context without rendering message contents', () => {
  const { rerender } = render(<QuotedContextSummary context={null} />);
  expect(screen.queryByLabelText('Reply context')).not.toBeInTheDocument();
  rerender(
    <QuotedContextSummary
      context={{
        relation: 'reply',
        text_available: true,
        text_truncated: true,
        content_kinds: ['image'],
      }}
    />,
  );
  expect(screen.getByLabelText('Reply context')).toHaveTextContent(
    'quoted excerpt available (truncated). Content: image.',
  );
  rerender(
    <QuotedContextSummary
      context={{
        relation: 'context',
        text_available: false,
        text_truncated: false,
        content_kinds: [],
      }}
    />,
  );
  expect(screen.getByLabelText('Reply context')).toHaveTextContent(
    'Native context: native message reference only.',
  );
});
