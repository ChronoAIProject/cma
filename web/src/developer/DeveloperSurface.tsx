import { useEffect, useRef, useState } from 'react';
import { safeHttpHref, type InlineLink } from '../chat/inline';
import { MarkdownContent } from '../markdown/MarkdownContent';
import { API_BASE_URL } from '../shell/build';
import {
  type SdkDocument,
  type SdkDocumentFilename,
  type SdkDocumentSets,
  type SdkLanguage,
} from './docSet';
import { SDK_DOCUMENTS } from './documents';
import './DeveloperSurface.css';

const SDK_LABELS: Readonly<Record<SdkLanguage, string>> = {
  typescript: 'TypeScript',
  rust: 'Rust',
};

interface DeveloperSurfaceProps {
  readonly documents?: SdkDocumentSets;
  readonly apiBaseUrl?: string;
}

interface PendingScroll {
  readonly filename: SdkDocumentFilename;
  readonly fragment?: string;
}

function firstDocument(documents: readonly SdkDocument[]): SdkDocument {
  const first = documents[0];
  if (first === undefined) {
    throw new Error('SDK documentation: validated guide set became empty');
  }
  return first;
}

function fragmentName(fragment: string | undefined): string | undefined {
  if (fragment === undefined || fragment === '') {
    return undefined;
  }
  try {
    return decodeURIComponent(fragment.replace(/^#/, ''));
  } catch {
    return fragment.replace(/^#/, '');
  }
}

export function DeveloperSurface({
  documents = SDK_DOCUMENTS,
  apiBaseUrl = API_BASE_URL,
}: DeveloperSurfaceProps) {
  const [sdk, setSdk] = useState<SdkLanguage>('typescript');
  const [filename, setFilename] = useState<SdkDocumentFilename>(
    () => firstDocument(documents.typescript).filename,
  );
  const pendingScroll = useRef<PendingScroll | undefined>(undefined);
  const scrollport = useRef<HTMLDivElement>(null);
  const article = useRef<HTMLElement>(null);
  const guides = documents[sdk];
  const current =
    guides.find((document) => document.filename === filename) ?? firstDocument(guides);

  const chooseDocument = (next: SdkDocumentFilename, fragment?: string) => {
    pendingScroll.current = { filename: next, ...(fragment === undefined ? {} : { fragment }) };
    setFilename(next);
  };

  const chooseSdk = (next: SdkLanguage) => {
    const nextGuides = documents[next];
    const nextDocument =
      nextGuides.find((document) => document.filename === current.filename) ??
      firstDocument(nextGuides);
    setSdk(next);
    chooseDocument(nextDocument.filename);
  };

  useEffect(() => {
    const pending = pendingScroll.current;
    if (pending === undefined || pending.filename !== current.filename) {
      return;
    }
    const fragment = fragmentName(pending.fragment);
    const target =
      fragment === undefined
        ? undefined
        : [...(article.current?.querySelectorAll<HTMLElement>('[id]') ?? [])].find(
            (element) => element.id === fragment,
          );
    if (target === undefined) {
      if (typeof scrollport.current?.scrollTo === 'function') {
        scrollport.current.scrollTo({ top: 0 });
      }
    } else if (typeof target.scrollIntoView === 'function') {
      target.scrollIntoView({ block: 'start' });
    }
    pendingScroll.current = undefined;
  }, [current.filename, sdk]);

  const resolveLink = (url: string): InlineLink | undefined => {
    const external = safeHttpHref(url);
    if (external !== undefined) {
      return { href: external, external: true };
    }
    const trimmed = url.trim();
    if (/^#[^#]*$/.test(trimmed)) {
      return { href: trimmed };
    }
    const local = /^(?:\.\/)?([^/?#]+\.md)(#[^#]*)?$/.exec(trimmed);
    if (local === null) {
      return undefined;
    }
    const target = guides.find((document) => document.filename === local[1]);
    if (target === undefined) {
      return undefined;
    }
    return {
      href: trimmed,
      onFollow: () => {
        chooseDocument(target.filename, local[2]);
      },
    };
  };

  return (
    <section className="developer" aria-label="Developer">
      <div className="sessionheader developer__head">
        <span className="sessionheader__crumb">Developer</span>
      </div>

      <div className="developer__scroll" ref={scrollport}>
        <div className="developer__layout">
          <aside className="developer__nav" aria-label="SDK documentation">
            <span className="developer__label">SDK</span>
            <div className="developer__sdk" role="group" aria-label="SDK">
              {(Object.keys(SDK_LABELS) as SdkLanguage[]).map((language) => (
                <button
                  key={language}
                  type="button"
                  className={`developer__sdk-key${sdk === language ? ' developer__sdk-key--on' : ''}`}
                  aria-pressed={sdk === language}
                  onClick={() => {
                    chooseSdk(language);
                  }}
                >
                  {SDK_LABELS[language]}
                </button>
              ))}
            </div>

            <span className="developer__label developer__guides-label">Guides</span>
            <nav className="developer__rail" aria-label={`${SDK_LABELS[sdk]} guides`}>
              {guides.map((document) => (
                <button
                  key={document.filename}
                  type="button"
                  className={`developer__document${
                    current.filename === document.filename ? ' developer__document--on' : ''
                  }`}
                  aria-current={current.filename === document.filename ? 'page' : undefined}
                  onClick={() => {
                    chooseDocument(document.filename);
                  }}
                >
                  {document.label}
                </button>
              ))}
            </nav>

            <label className="developer__picker">
              <span className="developer__label">Guide</span>
              <select
                value={current.filename}
                onChange={(event) => {
                  const next = guides.find((document) => document.filename === event.target.value);
                  if (next !== undefined) {
                    chooseDocument(next.filename);
                  }
                }}
              >
                {guides.map((document) => (
                  <option key={document.filename} value={document.filename}>
                    {document.label}
                  </option>
                ))}
              </select>
            </label>
          </aside>

          <div className="developer__reading">
            <dl className="developer__preface" role="group" aria-label="Deployment integration">
              <div className="developer__fact">
                <dt>API base</dt>
                <dd>
                  <code>{apiBaseUrl}</code>
                </dd>
              </div>
              <div className="developer__fact">
                <dt>Credential</dt>
                <dd>
                  Ask the deployment operator or identity administrator for a bearer credential.
                </dd>
              </div>
            </dl>

            <article className="developer__article" aria-label={current.title} ref={article}>
              <MarkdownContent
                key={`${sdk}:${current.filename}`}
                text={current.markdown}
                variant="developer"
                headingAnchors
                rawHtml="omit"
                resolveLink={resolveLink}
                renderImages={false}
                preserveUnknownInlineText={false}
              />
            </article>
          </div>
        </div>
      </div>
    </section>
  );
}
