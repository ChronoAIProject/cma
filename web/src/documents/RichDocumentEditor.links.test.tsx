import { fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import RichDocumentEditor from './RichDocumentEditor';

const source =
  '[Example](https://example.test/target) and [**Nested**](https://example.test/nested)';
afterEach(() => vi.restoreAllMocks());

function setup(editable = true) {
  const onChange = vi.fn();
  const onSave = vi.fn();
  const props = { source, editable, onChange, onSave };
  const rendered = render(<RichDocumentEditor {...props} />);
  const content = screen.getByRole('textbox', { name: 'Document content' });
  return {
    content,
    onChange,
    onSave,
    setEditable: (value: boolean) =>
      rendered.rerender(<RichDocumentEditor {...props} editable={value} />),
  };
}

const activations: [string, string, MouseEventInit][] = [
  ['plain click', 'click', { button: 0 }],
  ['Control click', 'click', { button: 0, ctrlKey: true }],
  ['Meta click', 'click', { button: 0, metaKey: true }],
  ['Shift click', 'click', { button: 0, shiftKey: true }],
  ['Alt click', 'click', { button: 0, altKey: true }],
  ['middle click', 'auxclick', { button: 1 }],
];

it.each(activations)(
  'cancels %s on links and nested marks only while editable',
  (_name, type, init) => {
    const { content, onChange, onSave, setEditable } = setup();
    const original = content;
    for (const editable of [true, false, true, false, true]) {
      setEditable(editable);
      expect(screen.getByRole('textbox', { name: 'Document content' })).toBe(original);
      expect(content).toHaveAttribute('contenteditable', String(editable));
      for (const target of [
        screen.getByRole('link', { name: 'Example' }),
        content.querySelector('strong')!,
      ]) {
        const event = new MouseEvent(type, { ...init, bubbles: true, cancelable: true });
        fireEvent(target, event);
        expect(event.defaultPrevented).toBe(editable);
      }
    }
    expect(onChange).not.toHaveBeenCalled();
    expect(onSave).not.toHaveBeenCalled();
    expect(content).toHaveTextContent('Example and Nested');
  },
);

it('allows links initially readonly and observes later changes into and out of Edit', () => {
  const { content, setEditable, onChange } = setup(false);
  const link = screen.getByRole('link', { name: 'Example' });
  for (const editable of [false, true, false]) {
    setEditable(editable);
    const event = new MouseEvent('click', { bubbles: true, cancelable: true });
    fireEvent(link, event);
    expect(event.defaultPrevented).toBe(editable);
    expect(link).toHaveAttribute('href', 'https://example.test/target');
    expect(link).toHaveAttribute('target', '_blank');
    expect(link).toHaveAttribute('rel', 'noopener noreferrer');
    expect(content).toHaveTextContent('Example and Nested');
  }
  expect(onChange).not.toHaveBeenCalled();
});

it('leaves plain text, context menus and links outside the content root alone', () => {
  const { content, onChange } = setup();
  const outside = document.createElement('a');
  outside.href = 'https://example.test/outside';
  outside.target = '_blank';
  content.parentElement!.append(outside);
  for (const [target, type, button] of [
    [content.querySelector('p')!, 'click', 0],
    [outside, 'click', 0],
    [screen.getByRole('link', { name: 'Example' }), 'auxclick', 2],
    [screen.getByRole('link', { name: 'Example' }), 'contextmenu', 2],
  ] as const) {
    const event = new MouseEvent(type, { bubbles: true, cancelable: true, button });
    fireEvent(target, event);
    expect(event.defaultPrevented).toBe(false);
  }
  expect(onChange).not.toHaveBeenCalled();
});

it.each(['ctrlKey', 'metaKey'])('retains the %s Save shortcut after link clicks', (modifier) => {
  const { content, onSave, onChange } = setup();
  fireEvent.click(screen.getByRole('link', { name: 'Example' }));
  const event = new KeyboardEvent('keydown', {
    key: 's',
    [modifier]: true,
    bubbles: true,
    cancelable: true,
  });
  fireEvent(content, event);
  expect(event.defaultPrevented).toBe(true);
  expect(onSave).toHaveBeenCalledTimes(1);
  expect(onChange).not.toHaveBeenCalled();
});
