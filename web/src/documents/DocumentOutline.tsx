import { useMemo } from 'react';
import { useEditorState, type Editor } from '@tiptap/react';

/** The editor's immutable document changes on edits, not on cursor movement. */
export function DocumentOutline({
  editor,
  open,
}: {
  readonly editor: Editor | null;
  readonly open: boolean;
}) {
  const document = useEditorState({
    editor,
    selector: ({ editor: current }) => current?.state.doc,
  });
  const headings = useMemo(() => {
    const items: { position: number; level: number; text: string }[] = [];
    document?.descendants((node, position) => {
      if (items.length >= 64) return false;
      if (node.type.name === 'heading')
        items.push({
          position,
          level: Number(node.attrs.level),
          text: node.textContent || 'Untitled heading',
        });
    });
    return items;
  }, [document]);
  return (
    <nav className="document-outline" aria-label="On this page" hidden={!open}>
      <p>On this page</p>
      {headings.length ? (
        <ol>
          {headings.map((heading) => (
            <li key={heading.position} data-level={heading.level}>
              <button
                type="button"
                title={heading.text}
                onClick={() => {
                  const node = editor?.view.nodeDOM(heading.position);
                  if (node instanceof HTMLElement)
                    node.scrollIntoView({
                      block: 'start',
                      behavior: window.matchMedia('(prefers-reduced-motion: reduce)').matches
                        ? 'instant'
                        : 'smooth',
                    });
                }}
              >
                {heading.text}
              </button>
            </li>
          ))}
        </ol>
      ) : (
        <span>Add headings to navigate your document.</span>
      )}
    </nav>
  );
}
