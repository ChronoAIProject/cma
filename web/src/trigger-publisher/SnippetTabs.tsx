import { useId, useRef, useState } from 'react';
import type { TriggerEmbed } from '@chronoai/cma-sdk';
import { Button } from '../ui/Button';
import { IconCopyOutline16 } from '../ui/icons';

const HOSTS = [
  {
    key: 'markdown',
    name: 'README or Markdown',
    guidance: 'Paste into the Markdown source of your README.',
  },
  { key: 'html', name: 'Web page', guidance: 'Paste into the HTML where the button belongs.' },
  {
    key: 'javascript',
    name: 'Page with JavaScript',
    guidance: 'Include the script once and place the custom element where the button belongs.',
  },
  { key: 'react', name: 'React app', guidance: 'Use this JSX inside your component.' },
  { key: 'url', name: 'Just the link', guidance: 'Use this URL as the destination of your link.' },
] as const;

export function CopyCode({ text, label }: { readonly text: string; readonly label: string }) {
  const [feedback, setFeedback] = useState('');
  const version = useRef(0);
  const copy = async () => {
    const current = ++version.current;
    try {
      await navigator.clipboard.writeText(text);
      if (current === version.current) setFeedback('Copied');
    } catch {
      if (current === version.current) setFeedback('Copy failed');
    }
  };
  return (
    <div className="trigger-code">
      <div className="trigger-code__header">
        <span>{label}</span>
        <span className="trigger-copy-feedback" role="status">
          {feedback}
        </span>
        <Button
          size="sm"
          icon={<IconCopyOutline16 />}
          aria-label={`Copy ${label}`}
          title={`Copy ${label}`}
          onClick={() => void copy()}
        />
      </div>
      <pre aria-label={label} tabIndex={0}>
        <code>{text}</code>
      </pre>
    </div>
  );
}

export function SnippetTabs({ descriptor }: { readonly descriptor: TriggerEmbed }) {
  const [selected, setSelected] = useState(0);
  const id = useId();
  const buttons = useRef<(HTMLButtonElement | null)[]>([]);
  const host = HOSTS[selected] ?? HOSTS[0];
  return (
    <div className="trigger-snippets">
      <div className="trigger-tabs" role="tablist" aria-label="Button host">
        {HOSTS.map((item, index) => (
          <button
            key={item.key}
            ref={(element) => {
              buttons.current[index] = element;
            }}
            id={`${id}-${item.key}`}
            type="button"
            role="tab"
            aria-selected={selected === index}
            aria-controls={`${id}-panel`}
            tabIndex={selected === index ? 0 : -1}
            onClick={() => setSelected(index)}
            onKeyDown={(event) => {
              const next =
                event.key === 'ArrowRight'
                  ? (index + 1) % HOSTS.length
                  : event.key === 'ArrowLeft'
                    ? (index + HOSTS.length - 1) % HOSTS.length
                    : event.key === 'Home'
                      ? 0
                      : event.key === 'End'
                        ? HOSTS.length - 1
                        : undefined;
              if (next !== undefined) {
                event.preventDefault();
                setSelected(next);
                buttons.current[next]?.focus();
              }
            }}
          >
            {item.name}
          </button>
        ))}
      </div>
      <div id={`${id}-panel`} role="tabpanel" aria-labelledby={`${id}-${host.key}`}>
        <CopyCode
          key={`${host.key}:${descriptor.public_url}`}
          text={descriptor.snippets[host.key]}
          label={`${host.name} snippet`}
        />
        <p className="trigger-caption">{host.guidance}</p>
        {host.key === 'javascript' ? (
          <div className="trigger-security">
            <CopyCode text={descriptor.javascript_sri} label="Subresource integrity" />
            <CopyCode
              text={descriptor.content_security_policy.img_src}
              label="Image CSP directive"
            />
            <CopyCode
              text={descriptor.content_security_policy.script_src}
              label="Script CSP directive"
            />
          </div>
        ) : null}
      </div>
    </div>
  );
}
