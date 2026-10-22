import { useEffect, useId, useState } from 'react';
import CodeBlock from '@tiptap/extension-code-block';
import {
  NodeViewContent,
  NodeViewWrapper,
  ReactNodeViewRenderer,
  type NodeViewProps,
} from '@tiptap/react';
import DOMPurify from 'dompurify';

function Diagram({ source }: { readonly source: string }) {
  const id = useId().replace(/[^a-zA-Z0-9]/g, '');
  const [rendered, setRendered] = useState<{ source: string; svg?: string; error?: string }>();
  useEffect(() => {
    let active = true;
    const timer = setTimeout(() => {
      void import('mermaid')
        .then(async ({ default: mermaid }) => {
          mermaid.initialize({
            startOnLoad: false,
            securityLevel: 'strict',
            htmlLabels: false,
            suppressErrorRendering: true,
            flowchart: { htmlLabels: false },
            maxTextSize: 20000,
            maxEdges: 200,
          });
          const { svg } = await mermaid.render(`cma-diagram-${id}`, source);
          if (active)
            setRendered({
              source,
              svg: DOMPurify.sanitize(svg, {
                USE_PROFILES: { svg: true, svgFilters: true },
                FORBID_TAGS: ['foreignObject', 'script'],
                FORBID_ATTR: ['onload', 'onclick'],
              }),
            });
        })
        .catch(() => {
          if (active)
            setRendered({
              source,
              error: 'Diagram preview is unavailable. Check the Mermaid syntax.',
            });
        });
    }, 250);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [source, id]);
  const current = rendered?.source === source ? rendered : undefined;
  return (
    <div className="document-diagram" contentEditable={false}>
      {current?.svg ? (
        <div
          role="img"
          aria-label="Mermaid diagram"
          dangerouslySetInnerHTML={{ __html: current.svg }}
        />
      ) : (
        <p role="status">{current?.error ?? 'Rendering diagram…'}</p>
      )}
    </div>
  );
}
function CodeBlockView({ node }: NodeViewProps) {
  return (
    <NodeViewWrapper className="document-code-block">
      <pre>
        <NodeViewContent<'code'> as="code" />
      </pre>
      {node.attrs.language === 'mermaid' ? <Diagram source={node.textContent} /> : null}
    </NodeViewWrapper>
  );
}
export const DocumentCodeBlock = CodeBlock.extend({
  addNodeView: () => ReactNodeViewRenderer(CodeBlockView),
});
