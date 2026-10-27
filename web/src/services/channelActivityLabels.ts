import type { ChannelActivityEvent } from './useChannelActivity';

export function activityLabel(event: ChannelActivityEvent): string {
  if (event.kind === 'event') {
    switch (event.event_type) {
      case 'cma.message.updated.v1':
        return 'Message edited';
      case 'cma.message.deleted.v1':
        return 'Message deleted';
      case 'cma.message.reaction.updated.v1':
        return 'Reaction updated';
    }
  }
  switch (event.kind) {
    case 'message':
      return 'Message received';
    case 'interaction':
      return 'Interaction received';
    case 'command':
      return 'Command received';
    default:
      return 'Event received';
  }
}

export function activityActor(event: ChannelActivityEvent): string {
  return event.actor.kind === 'unknown'
    ? 'Actor not reported'
    : (event.actor.display_name ?? event.actor.id);
}
