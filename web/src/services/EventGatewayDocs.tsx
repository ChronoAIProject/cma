import { memo } from 'react';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import Markdown from 'react-markdown';
import remarkGfm from 'remark-gfm';
import type { AppRoute } from '../routing/routes';
import integration from '../../../docs/CMAEG_Integration.md?raw';
import architecture from '../../../docs/CMAEG_Architecture.md?raw';
import protocol from '../../../docs/CMAEG_Protocol.md?raw';
import { GATEWAY_DIAGRAM_SIZES } from './gatewayDiagramSizes';
import './EventGatewayDocs.css';

export const GATEWAY_DOCUMENTS = [
  { id: 'integration', title: 'Integration guide', markdown: integration },
  { id: 'architecture', title: 'Architecture', markdown: architecture },
  { id: 'protocol', title: 'Protocol reference', markdown: protocol },
] as const;
const diagrams = import.meta.glob<string>('../../../api/crates/cmaeg/assets/docs/*.svg', {
  eager: true,
  query: '?url',
  import: 'default',
});
interface DocNode {
  type: string;
  depth?: number;
  value?: string;
  lang?: string;
  url?: string;
  alt?: string;
  children?: DocNode[];
  data?: { hProperties?: Record<string, unknown> };
}
const textOf = (node: DocNode): string => node.value ?? node.children?.map(textOf).join('') ?? '';
export function gatewayHeadingSlug(text: string) {
  return text
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, '')
    .trim()
    .split(/\s+/)
    .join('-');
}
/** Match the backend's section-scoped anchors and use its reviewed static diagrams. */
function documentPlugin(section: string) {
  return () => (value: unknown) => {
    const root = value as DocNode,
      counts = new Map<string, number>();
    let diagram = 0,
      heading = '';
    const walk = (node: DocNode) => {
      if (node.type === 'heading') {
        heading = textOf(node);
        const slug = gatewayHeadingSlug(heading),
          count = counts.get(slug) ?? 0;
        counts.set(slug, count + 1);
        node.data = {
          ...node.data,
          hProperties: {
            ...node.data?.hProperties,
            id: `${section}-${slug}${count ? `-${count}` : ''}`,
          },
        };
        node.depth = Math.min(6, (node.depth ?? 1) + 1);
      }
      if (node.type === 'code' && node.lang === 'mermaid') {
        const url = diagrams[`../../../api/crates/cmaeg/assets/docs/${section}-${diagram++}.svg`];
        if (url) {
          node.type = 'paragraph';
          node.children = [{ type: 'image', url, alt: `${heading} — service flow diagram` }];
          delete node.value;
          delete node.lang;
        }
      }
      node.children?.forEach(walk);
    };
    walk(root);
  };
}
export function gatewayDocLink(target: string, section: string): string | undefined {
  if (Object.values(diagrams).includes(target)) return target;
  if (
    target === '/openapi.json' ||
    target === '/event-gateway/openapi.json' ||
    target === 'https://bot.chrono-ai.fun/event-gateway/openapi.json' ||
    target === 'https://cma.nyxid.ai/event-gateway/openapi.json' ||
    target === 'https://bot-gateway.chrono-ai.fun/openapi.json'
  )
    return '/event-gateway/openapi.json';
  if (/^https?:\/\//.test(target)) {
    const url = new URL(target);
    if (
      (url.origin === 'https://bot-gateway.chrono-ai.fun' &&
        /^\/docs(?:\/|$)/.test(url.pathname)) ||
      (['https://bot.chrono-ai.fun', 'https://cma.nyxid.ai'].includes(url.origin) &&
        /^\/(?:event-gateway\/docs|docs\/event-gateway)(?:\/|$)/.test(url.pathname))
    ) {
      const page = url.pathname.split('/').at(-1);
      return page === 'architecture' || page === 'protocol'
        ? `#${page}${url.hash ? `-${url.hash.slice(1)}` : ''}`
        : url.hash || '#integration';
    }
    return target;
  }
  const [file = '', fragment] = target.split('#');
  const known: Record<string, string> = {
    'CMAEG_Integration.md': 'integration',
    'CMAEG_Architecture.md': 'architecture',
    'CMAEG_Protocol.md': 'protocol',
  };
  const id = file ? known[file] : section;
  return id ? `#${id}${fragment ? `-${fragment}` : ''}` : undefined;
}
function contents(markdown: string, section: string) {
  let fenced = false;
  const counts = new Map<string, number>();
  return markdown.split('\n').flatMap((line) => {
    if (line.startsWith('```')) {
      fenced = !fenced;
      return [];
    }
    const match = !fenced && /^(#{1,6}) (.+)$/.exec(line);
    if (!match) return [];
    const title = match[2]!.replace(/[`*_]/g, ''),
      slug = gatewayHeadingSlug(title),
      count = counts.get(slug) ?? 0;
    counts.set(slug, count + 1);
    return match[1]!.length === 2 || (match[1]!.length === 3 && title.startsWith('Flow '))
      ? [{ title, id: `${section}-${slug}${count ? `-${count}` : ''}` }]
      : [];
  });
}
const DOCUMENT_CONTENTS = GATEWAY_DOCUMENTS.map((document) => ({
  ...document,
  contents: contents(document.markdown, document.id),
  plugin: documentPlugin(document.id),
}));
function downloadMarkdown() {
  return (
    '# Agent Event Gateway\n\nIntegration guide, architecture and protocol reference.\n\n' +
    GATEWAY_DOCUMENTS.map((document) => {
      let fenced = false;
      const counts = new Map<string, number>();
      const annotated = document.markdown
        .split('\n')
        .map((line) => {
          if (line.startsWith('```')) fenced = !fenced;
          const match = !fenced && /^(#{1,6}) (.+)$/.exec(line);
          if (!match) return line;
          const slug = gatewayHeadingSlug(match[2]!.replace(/[`*_]/g, '')),
            count = counts.get(slug) ?? 0;
          counts.set(slug, count + 1);
          return `<a id="${document.id}-${slug}${count ? `-${count}` : ''}"></a>\n\n${line}`;
        })
        .join('\n')
        .replace(/\]\(([^)]+)\)/g, (original, target: string) => {
          const rewritten = gatewayDocLink(target, document.id);
          return rewritten ? `](${rewritten})` : original;
        });
      return `<a id="${document.id}"></a>\n\n${annotated}`;
    }).join('\n\n---\n\n')
  );
}
const download = `data:text/markdown;charset=utf-8,${encodeURIComponent(downloadMarkdown())}`;
const DocumentSection = memo(function DocumentSection({
  document,
}: {
  readonly document: (typeof DOCUMENT_CONTENTS)[number];
}) {
  return (
    <section id={document.id} aria-label={document.title}>
      <Markdown
        remarkPlugins={[remarkGfm, document.plugin]}
        skipHtml
        urlTransform={(target) => gatewayDocLink(target, document.id)}
        components={{
          a: ({ href, children }) =>
            href ? (
              <a
                href={href}
                {...(/^https?:/.test(href) ? { target: '_blank', rel: 'noopener noreferrer' } : {})}
              >
                {children}
              </a>
            ) : (
              <span>{children}</span>
            ),
          img: ({ src, alt }) => {
            const name = Object.entries(diagrams)
              .find(([, url]) => url === src)?.[0]
              .split('/')
              .at(-1)
              ?.replace('.svg', '');
            const size = name ? GATEWAY_DIAGRAM_SIZES[name] : undefined;
            return (
              <a
                className="gateway-docs__diagram"
                href={src}
                target="_blank"
                rel="noopener noreferrer"
                aria-label={`Open full-size ${alt ?? 'flow diagram'}`}
              >
                <img src={src} alt={alt} width={size?.[0]} height={size?.[1]} loading="lazy" />
              </a>
            );
          },
          table: ({ children }) => (
            <div className="gateway-docs__table">
              <table>{children}</table>
            </div>
          ),
        }}
      >
        {document.markdown}
      </Markdown>
    </section>
  );
});
export function DocsIndex({ onNavigate }: { readonly onNavigate?: (route: AppRoute) => void }) {
  useDocumentTitle('Docs');
  return (
    <section className="gateway-docs-index">
      <PageHeader
        breadcrumb={[{ label: 'Docs' }]}
        title="Docs"
        description="Guides and references for building with CMA."
      />
      <a
        className="gateway-docs-index__link"
        href="/docs/event-gateway"
        onClick={(event) => {
          if (
            onNavigate &&
            event.button === 0 &&
            !event.metaKey &&
            !event.ctrlKey &&
            !event.shiftKey &&
            !event.altKey
          ) {
            event.preventDefault();
            onNavigate({ kind: 'docs', view: 'event_gateway_protocol' });
          }
        }}
      >
        <h2>Event Gateway Protocol</h2>
        <p>Integrate event sources, manage channels, implement providers, and deliver replies.</p>
        <span>Read documentation →</span>
      </a>
    </section>
  );
}
export function EventGatewayDocs({
  onNavigate,
}: { readonly onNavigate?: (route: AppRoute) => void } = {}) {
  useDocumentTitle('Event Gateway Protocol');
  return (
    <div className="gateway-docs">
      <div className="gateway-docs__header">
        <PageHeader
          breadcrumb={[
            {
              label: 'Docs',
              href: '/docs',
              onSelect: onNavigate === undefined ? undefined : () => onNavigate({ kind: 'docs' }),
            },
            { label: 'Event Gateway Protocol' },
          ]}
          title="Event Gateway Protocol"
          description={
            <>
              <p>
                Connect event sources to agents. Build channels, integrate providers and deliver
                replies using one gateway.
              </p>
              <p>
                <a className="service-link" href={download} download="agent-event-gateway.md">
                  Download document
                </a>
                {' · '}
                <a className="service-link" href="/event-gateway/openapi.json">
                  OpenAPI JSON
                </a>
              </p>
            </>
          }
        />
      </div>
      <div className="gateway-docs__layout">
        <nav aria-label="On this page">
          {DOCUMENT_CONTENTS.map((document) => (
            <div key={document.id}>
              <a className="gateway-docs__section-link" href={`#${document.id}`}>
                {document.title}
              </a>
              {document.contents.map((item) => (
                <a href={`#${item.id}`} key={item.id}>
                  {item.title}
                </a>
              ))}
            </div>
          ))}
        </nav>
        <article>
          {DOCUMENT_CONTENTS.map((document) => (
            <DocumentSection key={document.id} document={document} />
          ))}
        </article>
      </div>
    </div>
  );
}
