import { render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import { DocumentTitleScope, useDocumentTitle } from './useDocumentTitle';

function Title({ value }: { readonly value?: string }) {
  useDocumentTitle(value);
  return null;
}

afterEach(() => {
  document.title = '';
});

it('names the tab after the view and restores the product name when it leaves', () => {
  const { rerender, unmount } = render(<Title value="Atlas" />);
  expect(document.title).toBe('Atlas · CMA');
  rerender(<Title value="Prove lemma" />);
  expect(document.title).toBe('Prove lemma · CMA');
  unmount();
  expect(document.title).toBe('CMA');
});

it('registers nothing for a name that is not known yet', () => {
  document.title = 'Before';
  render(<Title value={undefined} />);
  render(<Title value="   " />);
  expect(document.title).toBe('Before');
});

it('gives the title to the visible service and hands it back when that service hides', () => {
  const draw = (gateway: boolean) => (
    <>
      <DocumentTitleScope visible={!gateway}>
        <Title value="Prove lemma" />
      </DocumentTitleScope>
      <DocumentTitleScope visible={gateway}>
        <Title value="Event Gateway" />
      </DocumentTitleScope>
    </>
  );
  const { rerender } = render(draw(false));
  expect(document.title).toBe('Prove lemma · CMA');
  rerender(draw(true));
  expect(document.title).toBe('Event Gateway · CMA');
  rerender(draw(false));
  expect(document.title).toBe('Prove lemma · CMA');
});

it('keeps a nested scope hidden while its parent is hidden', () => {
  render(
    <>
      <Title value="Agent Manager" />
      <DocumentTitleScope visible={false}>
        <DocumentTitleScope visible>
          <Title value="Hidden" />
        </DocumentTitleScope>
      </DocumentTitleScope>
    </>,
  );
  expect(document.title).toBe('Agent Manager · CMA');
});
