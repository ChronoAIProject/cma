import Markdown, { type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import './MessageContent.css';

function safeHttpUrl(value: string | undefined): string | undefined {
  if (value === undefined) return undefined;
  try {
    const url = new URL(value);
    return url.protocol === 'http:' || url.protocol === 'https:' ? value : undefined;
  } catch {
    return undefined;
  }
}

const components: Components = {
  h1: ({ children }) => <h2>{children}</h2>,
  h2: ({ children }) => <h3>{children}</h3>,
  h3: ({ children }) => <h4>{children}</h4>,
  h4: ({ children }) => <h5>{children}</h5>,
  h5: ({ children }) => <h6>{children}</h6>,
  a: ({ href, children }) => {
    const safeHref = safeHttpUrl(href);
    return safeHref === undefined ? (
      <span>{children}</span>
    ) : (
      <a href={safeHref} target="_blank" rel="noopener noreferrer">
        {children}
      </a>
    );
  },
  img: ({ src, alt }) => {
    const safeSrc = safeHttpUrl(src);
    return safeSrc === undefined ? (
      <span>{alt}</span>
    ) : (
      <img src={safeSrc} alt={alt ?? ''} loading="lazy" referrerPolicy="no-referrer" />
    );
  },
  table: ({ children }) => (
    <div className="agent-content__table-wrap">
      <table>{children}</table>
    </div>
  ),
};

export function MessageContent({
  content,
  streaming = false,
}: {
  readonly content: string;
  readonly streaming?: boolean;
}) {
  return (
    <div className="agent-content" data-streaming={streaming ? '' : undefined}>
      {/* Default HTML handling preserves placeholders and markup as inert text. */}
      <Markdown remarkPlugins={[remarkGfm]} components={components}>
        {content}
      </Markdown>
    </div>
  );
}
