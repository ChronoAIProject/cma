import { AgentPreparationBoundary } from './AgentPreparationBoundary';
import type { AgentSurfaceProps } from './AgentSurfaceProps';
import { deriveRetiredPresentation } from './retiredPresentation';
import { RetainedAgentHost } from './RetainedAgentHost';
import './Agent.css';

/** Select a host before any active connection, suggestion or first-send hook mounts. */
export function AgentSurface(props: AgentSurfaceProps) {
  const terminal = deriveRetiredPresentation(props.selectedRetired, {
    viewerId: props.viewerId,
    authEpoch: props.firstSendContext?.authEpoch,
    agentId: props.agentId,
    authorized: props.accessAuthorized === true && props.accessUncertain !== true,
  });
  if (terminal.kind === 'retired') {
    const history = props.controller.retained;
    const matching =
      history &&
      history.status !== 'unconfirmed' &&
      Object.entries(terminal.identity).every(
        ([field, value]) => history.identity[field as keyof typeof history.identity] === value,
      );
    return (
      <RetainedAgentHost
        key={JSON.stringify(terminal.identity)}
        presentation={terminal}
        history={matching ? history : undefined}
        client={props.lifecycleClient}
        exportAllowed={props.permissions?.exportHistory === true}
        onPersonalHome={props.onPersonalHome ?? (() => props.onNavigate({ kind: 'home' }))}
        viewPosition={props.viewPosition}
        user={props.user}
      />
    );
  }
  if (props.controller.retained) {
    return (
      <section className="agent-surface">
        <p role="status">Checking access to the retained conversation.</p>
      </section>
    );
  }
  return <AgentPreparationBoundary {...props} />;
}
