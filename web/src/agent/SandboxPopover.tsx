import { useEffect, useRef, useState } from 'react';
import type { AgentDetail } from '../api/types';
import { Button } from '../ui/Button';
import { IconFollowsystemOutline16 } from '../ui/icons';
import { Popover } from '../ui/Popover';
import { Resources } from './Resources';
import type { SandboxResourceAvailability } from './sandboxInput';

/** Resource reads live only while the preview is open; hover never steals focus. */
export function SandboxPopover({
  detail,
  availability,
}: {
  readonly detail: AgentDetail;
  readonly availability: SandboxResourceAvailability;
}) {
  const [mode, setMode] = useState<'hover' | 'explicit'>();
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timer.current), []);
  return (
    <span
      className="agent-toolbar__machine"
      onPointerEnter={(event) => {
        clearTimeout(timer.current);
        if (event.pointerType === 'mouse') setMode((current) => current ?? 'hover');
      }}
      onPointerLeave={() => {
        if (mode === 'hover') timer.current = setTimeout(() => setMode(undefined), 150);
      }}
    >
      <Popover
        open={mode !== undefined}
        autoFocus={mode === 'explicit'}
        side="bottom"
        align="start"
        label="Sandbox machine"
        cardClassName="agent-toolbar__machine-card"
        onClose={() => {
          clearTimeout(timer.current);
          setMode(undefined);
        }}
        anchor={
          <Button
            size="sm"
            variant="ghost"
            icon={<IconFollowsystemOutline16 />}
            aria-label="Sandbox machine"
            title="Sandbox machine"
            aria-haspopup="dialog"
            aria-expanded={mode !== undefined}
            onClick={() => setMode((current) => (current === 'explicit' ? undefined : 'explicit'))}
          />
        }
      >
        <div className="agent-toolbar__machine-content">
          <div>
            <h2>Sandbox machine</h2>
            <code>{detail.sandbox.id}</code>
          </div>
          {mode === undefined ? null : (
            <Resources
              agentId={detail.id}
              sandboxId={detail.sandbox.id}
              sandboxProfile={detail.sandbox.profile}
              availability={availability}
            />
          )}
          <section aria-label="Enabled agent-key scope">
            <h2>Agent key scope</h2>
            {detail.reach.length ? (
              <ul className="agent-toolbar__scope">
                {[...new Set(detail.reach)].map((scope) => (
                  <li key={scope} title={scope}>
                    {scope}
                  </li>
                ))}
              </ul>
            ) : (
              <p>No services are enabled.</p>
            )}
          </section>
        </div>
      </Popover>
    </span>
  );
}
