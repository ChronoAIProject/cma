import { type Ref, useState, useSyncExternalStore } from 'react';
import { agentMascotAssets } from './agentMascotAssets';
import type { AgentVisual } from './agentVisual';
import './agent-mascot.css';

export type AgentMascotSize = 16 | 32 | 48 | 96;

interface AgentMascotProps {
  readonly visual: Pick<AgentVisual, 'action' | 'label'>;
  readonly size: AgentMascotSize;
  readonly ref?: Ref<HTMLSpanElement>;
}

function resolvedTheme() {
  return document.body.hasAttribute('data-ds-dark-theme') ? 'dark' : 'light';
}

function subscribeToTheme(notify: () => void) {
  const observer = new MutationObserver(notify);
  observer.observe(document.body, { attributeFilter: ['data-ds-dark-theme'] });
  return () => observer.disconnect();
}

function Poster({ src, size }: { readonly src: string; readonly size: AgentMascotSize }) {
  const [failed, setFailed] = useState(false);
  return failed ? (
    <span className="agent-mascot__fallback" />
  ) : (
    <img
      className="agent-mascot__poster"
      src={src}
      alt=""
      width={size}
      height={size}
      onError={() => setFailed(true)}
    />
  );
}

export function AgentMascot({ visual, size, ref }: AgentMascotProps) {
  const theme = useSyncExternalStore(subscribeToTheme, resolvedTheme);
  const { posters } = agentMascotAssets[visual.action][theme];
  const poster = posters[size === 96 ? 48 : size];
  const slotSize = size === 16 ? 24 : size;

  return (
    <span className="agent-mascot">
      <span
        ref={ref}
        className="agent-mascot__artwork"
        style={{ inlineSize: slotSize, blockSize: slotSize }}
        aria-hidden="true"
      >
        <Poster key={poster} src={poster} size={size} />
      </span>
      <span>{visual.label}</span>
    </span>
  );
}
