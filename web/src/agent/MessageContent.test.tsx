import { render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { MessageContent } from './MessageContent';

afterEach(() => vi.restoreAllMocks());

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

it('keeps native command placeholders visible while streaming and after completion', () => {
  const { container, rerender } = render(
    <MessageContent content="Use /goal <objective> to set a goal." streaming />,
  );
  expect(screen.getByText('Use /goal <objective> to set a goal.')).toBeVisible();
  expect(container.querySelector('.agent-content')).toHaveAttribute('data-streaming');
  rerender(
    <MessageContent
      content={'Use /goal <objective> to set a goal.\n\n**Example:** `/goal Fix the tests`'}
    />,
  );
  expect(screen.getByText('Use /goal <objective> to set a goal.')).toBeVisible();
  expect(container.querySelector('objective')).toBeNull();
  expect(container.querySelector('strong')).toHaveTextContent('Example:');
  expect(container.querySelector('code')).toHaveTextContent('/goal Fix the tests');
  expect(container.querySelector('.agent-content')).not.toHaveAttribute('data-streaming');
});

it.each([false, true])(
  'renders hostile raw markup as inert text and blocks unsafe Markdown URLs (streaming=%s)',
  (streaming) => {
    const alert = vi.spyOn(window, 'alert').mockImplementation(() => {});
    const script = '<script>window.alert("script")</script>';
    const image = '<img src="https://example.test/tracker" onerror="window.alert(1)">';
    const rawLink = '<a href="javascript:window.alert(2)">raw link</a>';
    const { container } = render(
      <MessageContent
        streaming={streaming}
        content={`${script}\n\n${image}\n\n${rawLink}\n\n[Unsafe link](javascript:alert(3))\n\n![Unsafe image](data:image/svg+xml;base64,PHN2Zz4=)`}
      />,
    );
    expect(container).toHaveTextContent(script);
    expect(container).toHaveTextContent(image);
    expect(container).toHaveTextContent(rawLink);
    expect(container.querySelector('script,img,svg,iframe,a,[onerror],[onclick]')).toBeNull();
    expect(screen.getByText('Unsafe link')).toBeVisible();
    expect(screen.getByText('Unsafe image')).toBeVisible();
    expect(alert).not.toHaveBeenCalled();
  },
);
