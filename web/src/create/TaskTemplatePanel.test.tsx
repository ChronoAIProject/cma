import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { emptyTemplate } from './taskTemplateModel';
import { TaskTemplatePanel } from './TaskTemplatePanel';

it.each([
  ['checking', 'Checking template availability…'],
  ['error', 'Template availability could not be checked.'],
  ['unsupported', 'Template tasks are not available on this service.'],
] as const)(
  'preserves fields and explains %s without substituting a different task',
  (availability, message) => {
    const template = { id: 'research', version: 1, fields: { topic: '保留研究 موضوع' } } as const;
    const onChange = vi.fn();
    render(
      <TaskTemplatePanel
        template={template}
        availability={availability}
        errorId="template-error"
        onChange={onChange}
      >
        <p>Compiled preview</p>
      </TaskTemplatePanel>,
    );
    expect(screen.getByRole('status')).toHaveTextContent(message);
    expect(
      screen.getByRole('status').compareDocumentPosition(screen.getByRole('group')) &
        Node.DOCUMENT_POSITION_FOLLOWING,
    ).toBeTruthy();
    expect(screen.getByLabelText('Topic')).toHaveValue('保留研究 موضوع');
    expect(onChange).not.toHaveBeenCalled();
    fireEvent.change(screen.getByLabelText('Topic'), { target: { value: 'Edited topic' } });
    expect(onChange).toHaveBeenCalledExactlyOnceWith({
      ...template,
      fields: { topic: 'Edited topic' },
    });
  },
);

it('associates missing-field feedback with the existing required input', () => {
  render(
    <TaskTemplatePanel
      template={emptyTemplate('document')}
      availability="supported"
      errorId="missing-subject"
      issue={{ field: 'subject', message: 'Enter subject before starting.' }}
      onChange={vi.fn()}
    >
      <p id="missing-subject">Enter subject before starting.</p>
    </TaskTemplatePanel>,
  );
  expect(screen.getByLabelText('Subject')).toBeRequired();
  expect(screen.getByLabelText('Subject')).toHaveAttribute('aria-invalid', 'true');
  expect(screen.getByLabelText('Subject')).toHaveAccessibleDescription(
    'Enter subject before starting.',
  );
  expect(screen.getByLabelText('Document type')).toHaveValue('');
});
