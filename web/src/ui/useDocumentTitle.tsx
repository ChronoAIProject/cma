import { createContext, useContext, useEffect, type ReactNode } from 'react';

/** The browser tab keeps the product's full name; view names belong in page headings.
 * Retained service scopes still gate title effects while their content is hidden.
 */

const VisibleScope = createContext(true);

/** Marks every title beneath it inactive while its service is hidden. */
export function DocumentTitleScope({
  visible,
  children,
}: {
  readonly visible: boolean;
  readonly children: ReactNode;
}) {
  const parent = useContext(VisibleScope);
  return <VisibleScope value={parent && visible}>{children}</VisibleScope>;
}

export const CMA_DOCUMENT_TITLE = 'ChronoAI Managed Agent (CMA)';

/** Existing callers may retain their view labels; they no longer rename browser tabs. */
export function useDocumentTitle(title: string | undefined) {
  const active = useContext(VisibleScope);
  useEffect(() => {
    if (active) document.title = CMA_DOCUMENT_TITLE;
  }, [title, active]);
}
