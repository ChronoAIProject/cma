import {
  Children,
  createContext,
  isValidElement,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from 'react';
import { Button } from '../ui/Button';
import { IconCheckOutline16, IconCopyOutline16 } from '../ui/icons';
import './CodeBlock.css';

/** Whether the reply holding a code block is still streaming, so its code is incomplete. */
export const CodeStreamingContext = createContext(false);

function textOf(node: ReactNode): string {
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  if (isValidElement<{ children?: ReactNode }>(node)) return textOf(node.props.children);
  return '';
}

/** The fence's info string as written, or nothing; a language is never inferred (§18.11). */
function languageOf(children: ReactNode): string | undefined {
  const code = Children.toArray(children)[0];
  if (!isValidElement<{ className?: string }>(code)) return undefined;
  return /(?:^|\s)language-(\S+)/.exec(code.props.className ?? '')?.[1];
}

/**
 * A fenced code block with its language and a Copy key (§13, §29). Copy writes the code
 * exactly as fenced, and stays unavailable while the reply is still arriving, because
 * half a command pasted into a shell is worse than none.
 */
export function CodeBlock({ children }: { readonly children?: ReactNode }) {
  const streaming = useContext(CodeStreamingContext);
  const [copy, setCopy] = useState<'copied' | 'failed'>();
  const language = languageOf(children);
  const text = textOf(children).replace(/\n$/, '');

  useEffect(() => {
    if (copy !== 'copied') return;
    const timer = window.setTimeout(() => setCopy(undefined), 2000);
    return () => window.clearTimeout(timer);
  }, [copy]);

  return (
    <div className="code-block">
      <div className="code-block__header">
        {language === undefined ? (
          <span />
        ) : (
          <span className="code-block__language">{language}</span>
        )}
        <Button
          size="sm"
          icon={copy === 'copied' ? <IconCheckOutline16 /> : <IconCopyOutline16 />}
          aria-label="Copy code"
          title={
            streaming
              ? 'Available when the reply finishes'
              : copy === 'copied'
                ? 'Copied'
                : 'Copy code'
          }
          disabled={streaming || text === ''}
          onClick={() => {
            void (async () => {
              try {
                if (!navigator.clipboard) throw new Error('Clipboard unavailable.');
                await navigator.clipboard.writeText(text);
                setCopy('copied');
              } catch {
                setCopy('failed');
              }
            })();
          }}
        />
        <span className="sr-only" role="status">
          {copy === 'copied' ? 'Code copied' : copy === 'failed' ? 'Copy failed' : ''}
        </span>
      </div>
      <pre className="code-block__pre">{children}</pre>
    </div>
  );
}
