import { useState } from 'react';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError, type TriggerDocument } from '@cma/client';
import { describe, expect, it, vi } from 'vitest';
import type { AppRoute } from '../routing/routes';
import type { TriggerPublisherClient } from './client';
import { TriggerPublisherPage } from './TriggerPublisherPage';
import { content, descriptor, draft, placement, published, publisherFixture } from './fixtures';

function RoutedPublisher({
  client,
  initial,
}: {
  readonly client: TriggerPublisherClient;
  readonly initial?: TriggerDocument;
}) {
  const [route, setRoute] = useState<AppRoute>(
    initial === undefined ? { kind: 'triggers' } : { kind: 'trigger', triggerId: initial.id },
  );
  const id =
    route.kind === 'trigger' || route.kind === 'trigger_embed' ? route.triggerId : undefined;
  return (
    <TriggerPublisherPage
      key={id ?? 'new'}
      client={client}
      triggerId={id}
      view={route.kind === 'trigger_embed' ? 'embed' : 'wizard'}
      onNavigate={setRoute}
    />
  );
}

async function next(user: ReturnType<typeof userEvent.setup>) {
  const button = screen.getByRole('button', { name: 'Next' });
  await waitFor(() => expect(button).toBeEnabled());
  await user.click(button);
}

async function toPresentation(user: ReturnType<typeof userEvent.setup>) {
  await next(user);
  await next(user);
  await screen.findByRole('textbox', { name: 'title' });
}

describe('Trigger publisher flow', () => {
  it('opens embed code from a saved published trigger and returns directly after adding a source', async () => {
    const user = userEvent.setup();
    const document = { ...published, placements: [] };
    const fixture = publisherFixture(document);
    render(<RoutedPublisher client={fixture.client} initial={document} />);
    await user.click(await screen.findByRole('button', { name: 'Get embed code' }));
    await screen.findByRole('heading', { name: 'Embed revision 1' });
    await user.click(screen.getByRole('button', { name: 'Add source URL' }));
    await screen.findByRole('textbox', { name: 'Source URL' });
    await user.type(screen.getByRole('textbox', { name: 'Source label' }), 'Web page');
    await user.type(
      screen.getByRole('textbox', { name: 'Source URL' }),
      'https://example.com/project',
    );
    await user.click(screen.getByRole('button', { name: 'Add source URL' }));
    await screen.findByText('plc_readme');
    await user.click(screen.getByRole('button', { name: 'Get embed code' }));
    await screen.findByRole('tab', { name: 'Web page' });
    expect(fixture.triggers.publish).not.toHaveBeenCalled();
  });

  it('makes published trigger rail links point to the embed route', async () => {
    const fixture = publisherFixture(published);
    const navigate = vi.fn();
    const user = userEvent.setup();
    render(<TriggerPublisherPage client={fixture.client} onNavigate={navigate} />);
    const link = await screen.findByRole('link', { name: /trureturing/ });
    expect(link).toHaveAttribute('href', `/triggers/manage/${published.id}/embed`);
    await user.click(link);
    expect(navigate).toHaveBeenCalledWith({ kind: 'trigger_embed', triggerId: published.id });
  });

  it.each([
    ['draft', 'Draft', 'muted'],
    ['published', 'Published', 'neutral'],
    ['retired', 'Retired', 'muted'],
  ] as const)(
    'marks a %s Trigger with its badge in the rail and the header',
    async (availability, word, tone) => {
      const document = { ...published, availability };
      const fixture = publisherFixture(document);
      render(
        <TriggerPublisherPage
          client={fixture.client}
          triggerId={document.id}
          view="embed"
          onNavigate={vi.fn()}
        />,
      );
      const link = await screen.findByRole('link', { name: /trureturing/ });
      expect(within(link).getByText(word).closest('.badge')).toHaveAttribute('data-tone', tone);
      const header = screen.getByRole('heading', { name: 'Triggers' }).closest('header')!;
      expect(within(header).getByText(word).closest('.badge')).toHaveAttribute('data-tone', tone);
      expect(within(link).queryByText(availability)).not.toBeInTheDocument();
    },
  );

  it('names no status in the header while authoring a new Trigger', async () => {
    const fixture = publisherFixture(published);
    render(<TriggerPublisherPage client={fixture.client} onNavigate={vi.fn()} />);
    await screen.findByRole('link', { name: /trureturing/ });
    const header = screen.getByRole('heading', { name: 'Triggers' }).closest('header')!;
    expect(header.querySelector('.badge')).toBeNull();
  });

  it('reports a list that could not be read as an alert with Try again', async () => {
    const fixture = publisherFixture();
    fixture.triggers.list.mockRejectedValueOnce(new TypeError('offline'));
    render(<TriggerPublisherPage client={fixture.client} onNavigate={vi.fn()} />);
    const alert = await screen.findByRole('alert');
    expect(alert).toHaveTextContent('Triggers could not be loaded.');
    expect(within(alert).getByRole('button', { name: 'Try again' })).toBeEnabled();
  });

  it('publishes the trureturing setup in five steps with pinned payloads and API README/HTML snippets', async () => {
    const user = userEvent.setup();
    const test = publisherFixture();
    render(<RoutedPublisher client={test.client} />);
    await screen.findByRole('heading', { name: 'What it opens' });
    expect(
      screen.getByText(
        'Visitors get an inline Agent from the system default image in an empty workspace.',
      ),
    ).toBeInTheDocument();
    await next(user);
    await user.click(screen.getByRole('radio', { name: 'Always start a new one' }));
    expect(screen.getByText(/Every click starts a new Agent there/)).toBeInTheDocument();
    await next(user);
    await user.type(screen.getByRole('textbox', { name: 'title' }), 'trureturing');
    await user.type(
      screen.getByRole('textbox', { name: 'description' }),
      'Contribute to trureturing',
    );
    await user.type(screen.getByRole('textbox', { name: 'First message' }), 'Inspect the project');
    expect(screen.getAllByRole('radio')).toHaveLength(4);
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('radio', { name: 'Always start a new one' })).toBeChecked();
    await next(user);
    expect(screen.getByRole('textbox', { name: 'title' })).toHaveValue('trureturing');
    await next(user);
    await user.type(screen.getByRole('textbox', { name: 'Source label' }), 'README');
    await user.type(screen.getByRole('textbox', { name: 'Source URL' }), placement.source_url);
    await user.click(screen.getByRole('button', { name: 'Add source URL' }));
    await screen.findByText('plc_readme');
    expect(test.triggers.create).toHaveBeenCalledOnce();
    expect(test.triggers.publish).not.toHaveBeenCalled();
    await next(user);
    expect(screen.getByRole('button', { name: 'Publish revision 1' })).toBeDisabled();
    await user.click(screen.getByRole('checkbox', { name: 'Publish revision 1 of trureturing' }));
    await user.click(screen.getByRole('button', { name: 'Publish revision 1' }));
    await screen.findByRole('heading', { name: 'Embed revision 1' });
    await screen.findByRole('tab', { name: 'README or Markdown' });
    expect(test.triggers.create.mock.calls[0]?.[0]).toMatchSnapshot('pinned publication content');
    expect(test.triggers.publish.mock.calls[0]?.slice(0, 2)).toMatchSnapshot(
      'exact draft publication',
    );
    expect(screen.getByLabelText('README or Markdown snippet').textContent).toBe(
      descriptor.snippets.markdown,
    );
    await user.click(screen.getByRole('tab', { name: 'Web page' }));
    expect(screen.getByLabelText('Web page snippet').textContent).toBe(descriptor.snippets.html);
  });

  it('retains an edited draft across a CAS conflict and a failed reconciliation read', async () => {
    const user = userEvent.setup();
    const test = publisherFixture(draft);
    render(<RoutedPublisher client={test.client} initial={draft} />);
    await screen.findByRole('heading', { name: 'What it opens' });
    await toPresentation(user);
    await user.clear(screen.getByRole('textbox', { name: 'title' }));
    await user.type(screen.getByRole('textbox', { name: 'title' }), 'Unsaved title');
    test.triggers.save.mockRejectedValueOnce(
      new ApiError(409, 'Draft changed', 'trigger_draft_changed'),
    );
    await user.click(screen.getByRole('button', { name: 'Save draft' }));
    await screen.findByText('The saved draft changed. Your changes are still here.');
    expect(screen.getByRole('textbox', { name: 'title' })).toHaveValue('Unsaved title');
    await user.click(screen.getByRole('button', { name: 'Load current draft' }));
    test.triggers.read.mockRejectedValueOnce(new Error('offline'));
    await user.click(screen.getByRole('button', { name: 'Load current draft' }));
    await screen.findByText('The current draft could not be loaded. Your changes are still here.');
    expect(screen.getByRole('textbox', { name: 'title' })).toHaveValue('Unsaved title');
    await user.click(screen.getByRole('button', { name: 'Load current draft' }));
    await waitFor(() =>
      expect(screen.getByRole('textbox', { name: 'title' })).toHaveValue(
        content.presentation.title,
      ),
    );
  });

  it('retries only the unresolved publish with its original key and saved version', async () => {
    const user = userEvent.setup();
    const test = publisherFixture(draft);
    render(<RoutedPublisher client={test.client} initial={draft} />);
    await screen.findByRole('heading', { name: 'What it opens' });
    await toPresentation(user);
    await user.type(screen.getByRole('textbox', { name: 'title' }), ' updated');
    await next(user);
    await next(user);
    test.triggers.publish.mockRejectedValueOnce(
      new ApiError(503, 'Uncertain', 'trigger_service_unavailable'),
    );
    await user.click(
      screen.getByRole('checkbox', { name: 'Publish revision 1 of trureturing updated' }),
    );
    await user.click(screen.getByRole('button', { name: 'Publish revision 1' }));
    await screen.findByRole('button', { name: 'Retry original request' });
    expect(screen.getByRole('button', { name: 'Save draft' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'New Trigger' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Your Triggers' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Retry original request' }));
    await screen.findByRole('heading', { name: 'Embed revision 1' });
    expect(test.triggers.save).toHaveBeenCalledOnce();
    const calls = vi.mocked(test.client.triggers.publish).mock.calls;
    expect(calls).toHaveLength(2);
    expect(calls[0]?.[1]).toEqual({ draft_version: 2 });
    expect(calls[1]?.[1]).toEqual(calls[0]?.[1]);
    expect(calls[1]?.[2].idempotencyKey).toBe(calls[0]?.[2].idempotencyKey);
  });

  it('shows server private-profile refusals beside the exact target picker', async () => {
    const user = userEvent.setup();
    const test = publisherFixture(draft);
    render(<RoutedPublisher client={test.client} initial={draft} />);
    await screen.findByRole('heading', { name: 'What it opens' });
    for (let step = 0; step < 4; step += 1) await next(user);
    test.triggers.publish.mockRejectedValueOnce(
      new ApiError(422, 'Private profile', 'trigger_reference_not_public', undefined, undefined, {
        field: 'target.workspace_profile',
      }),
    );
    await user.click(screen.getByRole('checkbox', { name: 'Publish revision 1 of trureturing' }));
    await user.click(screen.getByRole('button', { name: 'Publish revision 1' }));
    await screen.findByRole('heading', { name: 'What it opens' });
    const picker = screen.getByRole('button', { name: 'workspace profile' });
    expect(picker).toHaveAttribute('aria-invalid', 'true');
    expect(picker).toHaveAccessibleDescription(/Every referenced profile must be public/);
  });

  it('keeps placement create and retire keys through uncertain retries', async () => {
    const user = userEvent.setup();
    const test = publisherFixture(draft);
    render(<RoutedPublisher client={test.client} initial={draft} />);
    await screen.findByRole('heading', { name: 'What it opens' });
    for (let step = 0; step < 3; step += 1) await next(user);
    await user.type(screen.getByRole('textbox', { name: 'Source label' }), 'README');
    await user.type(screen.getByRole('textbox', { name: 'Source URL' }), placement.source_url);
    test.triggers.createPlacement.mockRejectedValueOnce(new ApiError(503, 'Uncertain'));
    await user.click(screen.getByRole('button', { name: 'Add source URL' }));
    await screen.findByRole('button', { name: 'Retry original request' });
    expect(screen.getByRole('textbox', { name: 'Source URL' })).toBeDisabled();
    await user.click(screen.getByRole('button', { name: 'Retry original request' }));
    await screen.findByText('plc_readme');
    const creates = vi.mocked(test.client.triggers.createPlacement).mock.calls;
    expect(creates).toHaveLength(2);
    expect(creates[0]?.[1]).toEqual(creates[1]?.[1]);
    expect(creates[0]?.[2].idempotencyKey).toBe(creates[1]?.[2].idempotencyKey);
    await user.click(screen.getByRole('button', { name: 'Remove README' }));
    test.triggers.retirePlacement.mockRejectedValueOnce(new ApiError(503, 'Uncertain'));
    await user.click(screen.getByRole('button', { name: 'Remove placement' }));
    await user.click(await screen.findByRole('button', { name: 'Retry original request' }));
    await waitFor(() => expect(screen.queryByText('plc_readme')).not.toBeInTheDocument());
    const retires = vi.mocked(test.client.triggers.retirePlacement).mock.calls;
    expect(retires).toHaveLength(2);
    expect(retires[0]?.[2].idempotencyKey).toBe(retires[1]?.[2].idempotencyKey);
  });

  it('retains a newly saved draft when placement creation is refused', async () => {
    const user = userEvent.setup();
    const test = publisherFixture();
    render(<RoutedPublisher client={test.client} />);
    await screen.findByRole('heading', { name: 'What it opens' });
    await toPresentation(user);
    await user.type(screen.getByRole('textbox', { name: 'title' }), 'trureturing');
    await next(user);
    await user.type(screen.getByRole('textbox', { name: 'Source label' }), 'README');
    await user.type(screen.getByRole('textbox', { name: 'Source URL' }), placement.source_url);
    test.triggers.createPlacement.mockRejectedValueOnce(new ApiError(422, 'Placement refused'));
    await user.click(screen.getByRole('button', { name: 'Add source URL' }));
    await screen.findByText('Placement refused');
    expect(test.triggers.create).toHaveBeenCalledOnce();
    await user.click(screen.getByRole('button', { name: 'Add source URL' }));
    await screen.findByText('plc_readme');
    expect(test.triggers.create).toHaveBeenCalledOnce();
    expect(
      within(screen.getByRole('navigation', { name: 'Your Triggers' })).getByText('trureturing'),
    ).toBeInTheDocument();
  });

  it('opens the saved draft route after an explicit first save without publishing', async () => {
    const user = userEvent.setup();
    const test = publisherFixture();
    const navigate = vi.fn();
    render(<TriggerPublisherPage client={test.client} onNavigate={navigate} />);
    await screen.findByRole('heading', { name: 'What it opens' });
    await toPresentation(user);
    await user.type(screen.getByRole('textbox', { name: 'title' }), 'Saved draft');
    await user.click(screen.getByRole('button', { name: 'Save draft' }));
    await waitFor(() =>
      expect(navigate).toHaveBeenCalledWith({ kind: 'trigger', triggerId: draft.id }),
    );
    expect(test.triggers.publish).not.toHaveBeenCalled();
  });

  it('offers explicit draft reconciliation when placement creation encounters a save conflict', async () => {
    const user = userEvent.setup();
    const test = publisherFixture(draft);
    render(<RoutedPublisher client={test.client} initial={draft} />);
    await screen.findByRole('heading', { name: 'What it opens' });
    await toPresentation(user);
    await user.type(screen.getByRole('textbox', { name: 'title' }), ' edited');
    await next(user);
    await user.type(screen.getByRole('textbox', { name: 'Source label' }), 'README');
    await user.type(screen.getByRole('textbox', { name: 'Source URL' }), placement.source_url);
    test.triggers.save.mockRejectedValueOnce(
      new ApiError(409, 'Conflict', 'trigger_draft_changed'),
    );
    await user.click(screen.getByRole('button', { name: 'Add source URL' }));
    await user.click(await screen.findByRole('button', { name: 'Load current draft' }));
    expect(test.triggers.createPlacement).not.toHaveBeenCalled();
    expect(screen.getByRole('textbox', { name: 'Source URL' })).toHaveValue(placement.source_url);
    await user.click(screen.getByRole('button', { name: 'Load current draft' }));
    await waitFor(() =>
      expect(screen.queryByRole('button', { name: 'Load current draft' })).not.toBeInTheDocument(),
    );
    await user.click(screen.getByRole('button', { name: 'Back' }));
    expect(screen.getByRole('textbox', { name: 'title' })).toHaveValue(content.presentation.title);
  });

  it('retains inline retirement confirmation and the original key across an uncertain outcome', async () => {
    const user = userEvent.setup();
    const test = publisherFixture(published);
    render(<RoutedPublisher client={test.client} initial={published} />);
    const retire = await screen.findByRole('button', { name: 'Retire Trigger' });
    await waitFor(() => expect(retire).toBeEnabled());
    await user.click(retire);
    expect(
      screen.getByText(/New launches will be refused; existing launches remain/),
    ).toBeInTheDocument();
    test.triggers.retire.mockRejectedValueOnce(new ApiError(503, 'Uncertain'));
    await user.click(screen.getByRole('button', { name: 'Retire Trigger' }));
    await user.click(await screen.findByRole('button', { name: 'Retry original request' }));
    await screen.findByText('Trigger retired');
    expect(test.triggers.retire).toHaveBeenCalledTimes(2);
    const calls = vi.mocked(test.client.triggers.retire).mock.calls;
    expect(calls[0]?.[1].idempotencyKey).toBe(calls[1]?.[1].idempotencyKey);
    expect(screen.getByRole('button', { name: 'Save draft' })).toBeDisabled();
  });
});
