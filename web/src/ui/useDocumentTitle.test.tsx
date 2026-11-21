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

it('keeps the full product title across view changes and unmount', () => {
  const { rerender, unmount } = render(<Title value="Atlas" />);
  expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
  rerender(<Title value="Prove lemma" />);
  expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
  unmount();
  expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
});

it('uses the product title even before a view name is known', () => {
  document.title = 'Before';
  render(<Title value={undefined} />);
  render(<Title value="   " />);
  expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
});

it('keeps the product title when retained services switch visibility', () => {
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
  expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
  rerender(draw(true));
  expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
  rerender(draw(false));
  expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
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
  expect(document.title).toBe('ChronoAI Managed Agent (CMA)');
});
