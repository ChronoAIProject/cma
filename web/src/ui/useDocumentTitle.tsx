import { createContext, useContext, useEffect, type ReactNode } from 'react';

/**
 * Name the browser tab after the view — `docs/Web_Style.md` §11.
 *
 * Tabs, history and screen-reader page announcements all read the document
 * title, so "CMA" on every route left somebody with five tabs unable to tell
 * them apart.
 *
 * Services stay mounted while hidden so their drafts survive a switch, which
 * means more than one view can want the title at once. Each view registers
 * its title instead of writing it; the most recently registered view that is
 * on screen wins, and a hidden service's scope marks its views inactive. When
 * a view leaves, the one behind it gets the title back.
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

interface Entry {
  readonly title: string;
  readonly active: boolean;
}

const entries: Entry[] = [];

function apply() {
  const current = [...entries].reverse().find((entry) => entry.active);
  document.title = current === undefined ? 'CMA' : `${current.title} · CMA`;
}

/** Pass undefined while the name is not known yet; nothing is registered for it. */
export function useDocumentTitle(title: string | undefined) {
  const active = useContext(VisibleScope);
  useEffect(() => {
    if (title === undefined || title.trim() === '') return;
    const entry: Entry = { title, active };
    entries.push(entry);
    apply();
    return () => {
      const at = entries.indexOf(entry);
      if (at !== -1) entries.splice(at, 1);
      apply();
    };
  }, [title, active]);
}
