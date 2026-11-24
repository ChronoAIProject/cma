import { useContext, useEffect, useRef, useState } from 'react';
import type { CmaWebClient } from '../api/client';
import { CharacterAvatar } from '../character/CharacterAvatar';
import { SidebarContext } from '../shell/sidebarContext';
import { Alert } from '../ui/Alert';
import { Button } from '../ui/Button';
import { IconPanelLeftOutline16 } from '../ui/icons';
import { PageHeader } from '../ui/PageHeader';
import { useDocumentTitle } from '../ui/useDocumentTitle';
import type { MessageUser } from './MessageIdentity';
import { OwnerMessageStream } from './OwnerMessageStream';
import { OriginalRequestReuse } from './OriginalRequestReuse';
import { agentDisplayTitle } from './presentation';
import type { RetainedAgentRead } from './retainedHistory';
import type { RetiredAgentPresentation } from './retiredPresentation';
import { useHistoryExport } from './useHistoryExport';
import type { AgentViewPosition } from './viewPosition';
import './RetainedAgentHost.css';

const refuseApproval = () => undefined;

/** No runtime or Agent mutation capability belongs to the retained host. */
export function RetainedAgentHost({
  presentation,
  history,
  client,
  exportAllowed,
  onPersonalHome,
  viewPosition,
  user,
}: {
  readonly presentation: RetiredAgentPresentation;
  readonly history?: RetainedAgentRead;
  readonly client?: CmaWebClient;
  readonly exportAllowed: boolean;
  readonly onPersonalHome: () => void;
  readonly viewPosition?: AgentViewPosition;
  readonly user?: MessageUser;
}) {
  const title = agentDisplayTitle(presentation.detail);
  const heading = useRef<HTMLHeadingElement>(null);
  const sidebar = useContext(SidebarContext);
  useDocumentTitle(title, 'task');
  useEffect(() => {
    heading.current?.focus({ preventScroll: true });
  }, []);
  return (
    <section className="agent-surface retained-agent">
      <div className="retained-agent__header">
        {sidebar?.collapsed ? (
          <Button
            size="sm"
            icon={<IconPanelLeftOutline16 />}
            aria-label="Show sidebar"
            onClick={sidebar.toggle}
          />
        ) : null}
        <CharacterAvatar pose="paused" still size={32} />
        <PageHeader title={title} headingRef={heading} wrapIdentity />
      </div>
      <div className="retained-agent__notice">
        <Alert tone="info">
          This agent&apos;s cloud computer was deleted. The conversation is kept read-only.
        </Alert>
      </div>
      <div className="agent-surface__conversation">
        {history?.status === 'ready' ? (
          <OwnerMessageStream
            retained
            conversation={history.conversation}
            client={client}
            agentId={presentation.identity.agentId}
            viewerId={presentation.identity.viewerId}
            authorized
            agentStatus="retired"
            emptyMessage="No conversation messages are available."
            arrivalsEnabled={false}
            approvalReadOnly
            approvalBusy
            onApproval={refuseApproval}
            viewPosition={viewPosition}
            user={user}
          />
        ) : history?.status === 'error' ? (
          <section>
            <Alert tone="warning" title="Conversation couldn’t be loaded">
              <p>{history.message}</p>
              <HistoryRetry retryAt={history.retryAt} retry={history.retry} />
            </Alert>
          </section>
        ) : (
          <section>
            <p role="status">Loading retained conversation.</p>
          </section>
        )}
      </div>
      <div className="retained-agent__footer">
        <p className="retained-agent__coverage">
          History shows the records still available. Raw runtime and tool records are not included.
        </p>
        {client ? (
          <>
            <RetainedExport client={client} presentation={presentation} allowed={exportAllowed} />
            <OriginalRequestReuse
              client={client}
              identity={presentation.identity}
              onImported={onPersonalHome}
            />
          </>
        ) : (
          <p>Conversation export is unavailable with this access.</p>
        )}
      </div>
    </section>
  );
}

function HistoryRetry({
  retryAt,
  retry,
}: {
  readonly retryAt: number;
  readonly retry: () => void;
}) {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const delay = Math.max(0, retryAt - Date.now());
    const timer = window.setTimeout(() => setNow(Date.now()), delay);
    return () => window.clearTimeout(timer);
  }, [retryAt]);
  return (
    <Button variant="outline" onClick={retry} disabled={now < retryAt}>
      Load conversation again
    </Button>
  );
}

function RetainedExport({
  client,
  presentation,
  allowed,
}: {
  readonly client: CmaWebClient;
  readonly presentation: RetiredAgentPresentation;
  readonly allowed: boolean;
}) {
  const [authority] = useState(() => ({}));
  const history = useHistoryExport(client, presentation.identity.viewerId, allowed, authority);
  return (
    <>
      <Button
        variant="outline"
        disabled={!allowed || history.pending}
        onClick={() => void history.run(presentation.identity.agentId, true, 'transcript')}
      >
        {history.pending ? 'Exporting conversation…' : 'Export conversation'}
      </Button>
      {!allowed ? <p>Conversation export is unavailable with this access.</p> : null}
      {history.error ? <p role="alert">{history.error}</p> : null}
      {history.notice ? <p role="status">{history.notice}</p> : null}
    </>
  );
}
