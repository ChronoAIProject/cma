import { chooseOptionValue } from '../test/select';
import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import { EmbedPanel } from './EmbedPanel';
import { CopyCode, SnippetTabs } from './SnippetTabs';
import { descriptor, published, publisherFixture } from './fixtures';

describe('Trigger embed panel', () => {
  it('renders every API golden snippet and security string verbatim with keyboard tabs', async () => {
    const user = userEvent.setup();
    render(<SnippetTabs descriptor={descriptor} />);
    const hosts = [
      ['README or Markdown', 'markdown'],
      ['Web page', 'html'],
      ['Page with JavaScript', 'javascript'],
      ['React app', 'react'],
      ['Just the link', 'url'],
    ] as const;
    for (const [name, key] of hosts) {
      await user.click(screen.getByRole('tab', { name }));
      expect(screen.getByLabelText(`${name} snippet`).textContent).toBe(descriptor.snippets[key]);
      if (key === 'javascript') {
        expect(screen.getByLabelText('Subresource integrity').textContent).toBe(
          descriptor.javascript_sri,
        );
        expect(screen.getByLabelText('Image CSP directive').textContent).toBe(
          descriptor.content_security_policy.img_src,
        );
        expect(screen.getByLabelText('Script CSP directive').textContent).toBe(
          descriptor.content_security_policy.script_src,
        );
      }
    }
    await user.keyboard('{Home}');
    expect(screen.getByRole('tab', { name: 'README or Markdown' })).toHaveFocus();
    await user.keyboard('{ArrowRight}');
    expect(screen.getByRole('tab', { name: 'Web page' })).toHaveAttribute('aria-selected', 'true');
  });
  it('copies exact source and reports copy failures without claiming success', async () => {
    const user = userEvent.setup();
    const copy = vi.spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined);
    render(<CopyCode text={descriptor.snippets.html} label="HTML" />);
    await user.click(screen.getByRole('button', { name: 'Copy HTML' }));
    expect(copy).toHaveBeenCalledWith(descriptor.snippets.html);
    expect(screen.getByRole('status')).toHaveTextContent('Copied');
    copy.mockRejectedValueOnce(new Error('denied'));
    await user.click(screen.getByRole('button', { name: 'Copy HTML' }));
    expect(screen.getByRole('status')).toHaveTextContent('Copy failed');
  });
  it('loads the selected placement and real badge variants with nonzero statistics', async () => {
    const test = publisherFixture(published);
    render(<EmbedPanel client={test.client} document={published} onManageSources={vi.fn()} />);
    await screen.findByRole('tab', { name: 'README or Markdown' });
    expect(test.triggers.embed).toHaveBeenCalledWith(
      published.id,
      'plc_readme',
      expect.objectContaining({ signal: expect.any(AbortSignal) }),
    );
    const images = screen.getAllByRole('img', { name: descriptor.label_text });
    expect(images).toHaveLength(2);
    expect(new URL(images[0]!.getAttribute('src')!).searchParams.get('theme')).toBe('light');
    expect(new URL(images[1]!.getAttribute('src')!).searchParams.get('theme')).toBe('dark');
    await waitFor(() =>
      expect(screen.getByLabelText('Trigger statistics')).toHaveTextContent('12'),
    );
    expect(screen.getByLabelText('Trigger statistics')).toHaveTextContent('7');
    expect(screen.getByLabelText('Trigger statistics')).toHaveTextContent('9');
  });
  it('does not show stale snippets while a different placement is loading', async () => {
    const user = userEvent.setup();
    const test = publisherFixture(published);
    const document = {
      ...published,
      placements: [
        ...published.placements,
        { id: 'plc_web', label: 'Website', source_url: 'https://example.com' },
      ],
    };
    render(<EmbedPanel client={test.client} document={document} onManageSources={vi.fn()} />);
    await screen.findByRole('tab', { name: 'README or Markdown' });
    test.triggers.embed.mockImplementationOnce(() => new Promise(() => undefined));
    await chooseOptionValue(user, screen.getByRole('button', { name: 'Placement' }), 'plc_web');
    expect(screen.getByText('Loading embed')).toBeInTheDocument();
    expect(screen.queryByLabelText('README or Markdown snippet')).not.toBeInTheDocument();
  });
  it('distinguishes unavailable statistics and no placement from zero', async () => {
    const test = publisherFixture(published);
    test.triggers.stats.mockRejectedValueOnce(new Error('offline'));
    render(
      <EmbedPanel
        client={test.client}
        document={{ ...published, placements: [] }}
        onManageSources={vi.fn()}
      />,
    );
    await screen.findByRole('button', { name: 'Retry statistics' });
    expect(screen.getByLabelText('Trigger statistics')).toHaveTextContent('No placement');
    expect(screen.getAllByText('Unavailable')).toHaveLength(2);
    expect(test.triggers.embed).not.toHaveBeenCalled();
  });
});
