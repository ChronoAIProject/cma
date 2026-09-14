import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { MessageContent } from './MessageContent';

it('renders GFM structure without activating unsafe links', () => {
  render(
    <MessageContent
      content={`## Result

- [x] Checked
- Uses \`npm test\`

| Gate | Result |
| --- | --- |
| Tests | Pass |

[Report](https://example.test/report) [Unsafe](javascript:alert(1))`}
    />,
  );

  expect(screen.getByRole('heading', { name: 'Result', level: 3 })).toBeInTheDocument();
  expect(screen.getByRole('checkbox')).toBeChecked();
  expect(screen.getByRole('cell', { name: 'Pass' })).toBeInTheDocument();
  expect(screen.getByRole('link', { name: 'Report' })).toHaveAttribute(
    'href',
    'https://example.test/report',
  );
  expect(screen.queryByRole('link', { name: 'Unsafe' })).not.toBeInTheDocument();
  expect(screen.getByText('Unsafe')).toBeInTheDocument();
});
