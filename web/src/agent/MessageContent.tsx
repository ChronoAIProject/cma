import { memo, useContext } from 'react';
import Markdown, { defaultUrlTransform, type Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import remarkCjkFriendly from 'remark-cjk-friendly/parseOnly';
import { navigateAppPath } from '../routing/navigationGuard';
import { MessageMedia } from './MessageMedia';
import {
  MessageMediaContext,
  messageFilePath,
  messageMediaType,
  safeHttpUrl,
} from './mediaSources';
import { CodeBlock, CodeStreamingContext } from './CodeBlock';
import { remarkLinkSemicolon } from './remarkLinkSemicolon';
import './MessageContent.css';

const components: Components = {
  h1: ({ children }) => <h2>{children}</h2>,
  h2: ({ children }) => <h3>{children}</h3>,
  h3: ({ children }) => <h4>{children}</h4>,
  h4: ({ children }) => <h5>{children}</h5>,
  h5: ({ children }) => <h6>{children}</h6>,
  a: function MessageLink({ href, children }) {
    const access = useContext(MessageMediaContext);
    let documentHref: string | undefined;
    if (href) {
      try {
        const url = new URL(href, window.location.origin);
        if (
          url.origin === window.location.origin &&
          /^\/artifacts\/doc_[a-f0-9]{32}$/.test(url.pathname) &&
          !url.search
        )
          documentHref = url.pathname + url.hash;
      } catch {
        /* External links keep the existing URL policy. */
      }
    }
    if (documentHref)
      return (
        <a
          href={documentHref}
          onClick={(event) => {
            if (event.button || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey)
              return;
            event.preventDefault();
            navigateAppPath(documentHref);
          }}
        >
          {children}
        </a>
      );
    const safeHref = safeHttpUrl(href);
    const path = messageFilePath(href, access?.workingDirectory);
    if (href && (safeHref || (access && path)) && messageMediaType(safeHref ?? path ?? '')) {
      return <MessageMedia src={href} label={typeof children === 'string' ? children : 'Media'} />;
    }
    return safeHref === undefined ? (
      <span>{children}</span>
    ) : (
      <a href={safeHref} target="_blank" rel="noopener noreferrer">
        {children}
      </a>
    );
  },
  img: ({ src, alt }) => <MessageMedia src={src ?? ''} label={alt ?? ''} image />,
  pre: ({ children }) => <CodeBlock>{children}</CodeBlock>,
  table: ({ children }) => (
    <div className="agent-content__table-wrap">
      <table>{children}</table>
    </div>
  ),
};

const inertComponents: Components = {
  ...components,
  a: ({ children }) => <span>{children}</span>,
  img: ({ alt }) => <span>{alt}</span>,
};
export const MessageContent = memo(function MessageContent({
  content,
  streaming = false,
  inertResources = false,
}: {
  readonly content: string;
  readonly streaming?: boolean;
  readonly inertResources?: boolean;
}) {
  const access = useContext(MessageMediaContext);
  return (
    <CodeStreamingContext.Provider value={streaming}>
      <div className="agent-content" data-streaming={streaming ? '' : undefined}>
        {/* Default HTML handling preserves placeholders and markup as inert text. */}
        <Markdown
          remarkPlugins={[remarkGfm, remarkCjkFriendly, remarkLinkSemicolon]}
          components={inertResources ? inertComponents : components}
          urlTransform={(url) =>
            messageFilePath(url, access?.workingDirectory) === undefined
              ? defaultUrlTransform(url)
              : url
          }
        >
          {content}
        </Markdown>
      </div>
    </CodeStreamingContext.Provider>
  );
});
