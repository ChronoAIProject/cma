import { useState } from 'react';
import { fireEvent, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it } from 'vitest';
import type { TriggerQuickMessage } from '@cma/client';
import { QuickMessageFields } from './QuickMessageFields';
import { blankContent, validateStep } from './model';

it('edits all three fields, preserves adjacent messages and removes a chosen message', async () => {
  function Editor() {
    const [value, setValue] = useState<readonly TriggerQuickMessage[]>([]);
    return <QuickMessageFields value={value} disabled={false} onChange={setValue} />;
  }
  const actor = userEvent.setup();
  render(<Editor />);
  await actor.click(screen.getByRole('button', { name: 'Add quick message' }));
  const first = within(screen.getByRole('group', { name: 'Quick message 1' }));
  for (const [label, value] of [
    ['Name', 'Introduce'],
    ['Description', 'Describe this project.'],
    ['Message text', 'Read the introduction.'],
  ] as const)
    fireEvent.change(first.getByLabelText(label), { target: { value } });
  await actor.click(screen.getByRole('button', { name: 'Add quick message' }));
  await actor.click(screen.getByRole('button', { name: 'Remove quick message 2' }));
  expect(first.getByLabelText('Name')).toHaveValue('Introduce');
  expect(first.getByLabelText('Description')).toHaveValue('Describe this project.');
  expect(first.getByLabelText('Message text')).toHaveValue('Read the introduction.');
});

it('rejects empty, duplicate and oversized shortcut fields, including multibyte text', () => {
  const content = blankContent();
  const message = {
    name: 'Introduce',
    description: 'Project introduction',
    text: 'Introduce the project',
  };
  const check = (quick_messages: readonly TriggerQuickMessage[]) =>
    validateStep(2, { ...content, quick_messages }, {});
  expect(check([message]).quick_messages).toBeUndefined();
  expect(check([{ ...message, text: ' ' }]).quick_messages).toBeDefined();
  expect(check([{ ...message, description: ' ' }]).quick_messages).toBeDefined();
  expect(check([{ ...message, name: ' ' }]).quick_messages).toBeDefined();
  expect(check([{ ...message, name: '\u754c'.repeat(41) }]).quick_messages).toBeDefined();
  expect(check([message, { ...message, name: ' INTRODUCE ' }]).quick_messages).toMatch(
    /different name/,
  );
});
