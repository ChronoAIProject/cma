import { createContext, useContext, useEffect, type ReactNode } from 'react';

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

export function useDocumentTitle(
  title: string | undefined,
  mode: 'product' | 'task' | 'inherit' = 'product',
) {
  const active = useContext(VisibleScope);
  useEffect(() => {
    if (active && mode !== 'inherit') {
      document.title =
        mode === 'task' && title?.trim() ? `${title.trim()} · CMA` : CMA_DOCUMENT_TITLE;
    }
  }, [title, active, mode]);
}
