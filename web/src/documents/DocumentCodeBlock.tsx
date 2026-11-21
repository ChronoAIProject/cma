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
  const [dark, setDark] = useState(() => document.body.hasAttribute('data-ds-dark-theme'));
  const [rendered, setRendered] = useState<{
    source: string;
    dark: boolean;
    svg?: string;
    error?: string;
  }>();
  useEffect(() => {
    const observer = new MutationObserver(() =>
      setDark(document.body.hasAttribute('data-ds-dark-theme')),
    );
    observer.observe(document.body, {
      attributes: true,
      attributeFilter: ['data-ds-dark-theme'],
    });
    return () => observer.disconnect();
  }, []);
  useEffect(() => {
    let active = true;
    const timer = setTimeout(() => {
      void import('mermaid')
        .then(async ({ default: mermaid }) => {
          const styles = getComputedStyle(document.body);
          const color = (alias: string) => styles.getPropertyValue(`--dsw-alias-${alias}`).trim();
          mermaid.initialize({
            theme: 'base',
            themeVariables: {
              darkMode: dark,
              dropShadow: 'none',
              background: color('bg-layer-1'),
              primaryColor: color('bg-layer-2'),
              primaryTextColor: color('label-primary'),
              primaryBorderColor: color('border-l3'),
              secondaryColor: color('bg-layer-3'),
              secondaryTextColor: color('label-primary'),
              tertiaryColor: color('bg-layer-1'),
              tertiaryTextColor: color('label-primary'),
              lineColor: color('label-secondary'),
              textColor: color('label-primary'),
              mainBkg: color('bg-layer-2'),
              clusterBkg: color('bg-layer-1'),
              clusterBorder: color('border-l2'),
              edgeLabelBackground: color('bg-layer-1'),
              fontFamily: styles.fontFamily,
            },
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
              dark,
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
              dark,
              error: 'Diagram preview is unavailable. Check the Mermaid syntax.',
            });
        });
    }, 250);
    return () => {
      active = false;
      clearTimeout(timer);
    };
  }, [source, id, dark]);
  const current = rendered?.source === source && rendered.dark === dark ? rendered : undefined;
  return (
    <div className="document-diagram" contentEditable={false}>
      {current?.svg ? (
        <img
          alt="Mermaid diagram"
          src={`data:image/svg+xml;charset=utf-8,${encodeURIComponent(current.svg)}`}
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
