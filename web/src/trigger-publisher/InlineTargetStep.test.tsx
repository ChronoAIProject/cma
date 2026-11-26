import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import type { TriggerInlineTarget } from '@cma/client';
import { TriggerWizard } from './TriggerWizard';
import { draft, publisherFixture } from './fixtures';
import { blankContent } from './model';

afterEach(() => vi.unstubAllGlobals());

function show(retainedImage?: TriggerInlineTarget['image']) {
  const content = blankContent();
  const inline = content.target.inline;
  const target =
    retainedImage === undefined || inline === undefined
      ? content.target
      : { ...content.target, inline: { ...inline, image: retainedImage } };
  const document = {
    ...draft,
    draft: { ...content, target, presentation: draft.draft.presentation },
  };
  const fixture = publisherFixture(document);
  const wizard = (
    <TriggerWizard
      client={fixture.client}
      document={document}
      onChange={vi.fn()}
      onPublished={vi.fn()}
      onLock={vi.fn()}
      onSaved={vi.fn()}
    />
  );
  render(wizard);
  return fixture;
}

it('preserves a legacy Trigger image when saving metadata', async () => {
  const fetcher = vi.fn();
  vi.stubGlobal('fetch', fetcher);
  const retained = {
    id: 'img_custom',
    version: 'build_one',
    reference: `registry.example/runtime@sha256:${'a'.repeat(64)}`,
  };
  const fixture = show(retained);
  const user = userEvent.setup();
  await user.click(await screen.findByRole('button', { name: 'Next' }));
  await user.click(screen.getByRole('button', { name: 'Next' }));
  await user.clear(screen.getByLabelText('Title'));
  await user.type(screen.getByLabelText('Title'), 'Updated title');
  expect(screen.queryByRole('button', { name: 'Image' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  await waitFor(() => expect(fixture.triggers.save).toHaveBeenCalledOnce());
  expect(fixture.triggers.save.mock.calls[0]?.[1].content.target.inline).toMatchObject({
    image: retained,
  });
  expect(fetcher).not.toHaveBeenCalled();
});

it('saves the deployment default for new Trigger drafts', async () => {
  const fixture = show();
  const user = userEvent.setup();
  const model = await screen.findByRole('textbox', { name: 'Model' });
  await user.type(model, 'my-model');
  expect(screen.queryByRole('button', { name: 'Image' })).not.toBeInTheDocument();
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  await waitFor(() => expect(fixture.triggers.save).toHaveBeenCalledOnce());
  const inline = fixture.triggers.save.mock.calls[0]?.[1].content.target.inline;
  expect(inline?.image).toBeUndefined();
  expect(inline?.agent.model).toBe('my-model');
});
