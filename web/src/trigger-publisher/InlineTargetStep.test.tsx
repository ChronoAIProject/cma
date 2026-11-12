import { render, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { TriggerWizard } from './TriggerWizard';
import { draft, publisherFixture } from './fixtures';
import { blankContent } from './model';

function show(customImage = false) {
  const content = blankContent();
  const document = {
    ...draft,
    draft: {
      ...content,
      target: {
        ...content.target,
        inline: {
          ...content.target.inline!,
          ...(customImage ? { image: { id: 'img_retained' } } : {}),
        },
      },
      presentation: draft.draft.presentation,
    },
  };
  const fixture = publisherFixture(document);
  render(
    <TriggerWizard
      client={fixture.client}
      document={document}
      onChange={vi.fn()}
      onPublished={vi.fn()}
      onLock={vi.fn()}
      onSaved={vi.fn()}
    />,
  );
  return fixture;
}

it('saves inline Agent edits with the system default image and no image selector', async () => {
  const fixture = show();
  const user = userEvent.setup();
  expect(screen.getByText('System default image')).toBeInTheDocument();
  expect(screen.queryByRole('button', { name: 'Image' })).not.toBeInTheDocument();
  const model = screen.getByRole('textbox', { name: 'Model' });
  await waitFor(() => expect(model).toBeEnabled());
  await user.type(model, 'my-model');
  await user.type(screen.getByRole('textbox', { name: 'System prompt' }), 'Keep my edits');
  await user.click(screen.getByRole('button', { name: 'Save draft' }));
  await waitFor(() => expect(fixture.triggers.save).toHaveBeenCalledOnce());
  const target = fixture.triggers.save.mock.calls[0]?.[1].content.target.inline;
  expect(target).not.toHaveProperty('image');
  expect(target?.agent).toMatchObject({ model: 'my-model', system_prompt: 'Keep my edits' });
  for (let step = 0; step < 4; step += 1) {
    const next = screen.getByRole('button', { name: 'Next' });
    await waitFor(() => expect(next).toBeEnabled());
    await user.click(next);
  }
  expect(screen.getByRole('heading', { name: 'Publish' })).toBeInTheDocument();
  expect(screen.getByText('System default image')).toBeInTheDocument();
  expect(screen.getByText('my-model')).toBeInTheDocument();
  expect(screen.getByText('Keep my edits')).toBeInTheDocument();
  expect(screen.queryByText(/Profile unavailable/)).not.toBeInTheDocument();
  expect(screen.queryByRole('link', { name: /profile/i })).not.toBeInTheDocument();
});

it('requires switching a retained custom image draft to the system default before saving', async () => {
  const fixture = show(true);
  const user = userEvent.setup();
  const save = screen.getByRole('button', { name: 'Save draft' });
  await waitFor(() => expect(save).toBeEnabled());
  await user.click(save);
  expect(screen.getByRole('alert')).toHaveTextContent('Custom image selection is unsupported');
  expect(fixture.triggers.save).not.toHaveBeenCalled();
  await user.click(screen.getByRole('button', { name: 'Use system default image' }));
  await user.click(save);
  await waitFor(() => expect(fixture.triggers.save).toHaveBeenCalledOnce());
  expect(fixture.triggers.save.mock.calls[0]?.[1].content.target.inline?.image).toBeUndefined();
});
