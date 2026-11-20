import { useLayoutEffect, useRef, useState } from 'react';
import { registerNavigationGuard } from '../routing/navigationGuard';
import { parseRoute } from '../routing/routes';
import { Button } from '../ui/Button';
import { Modal } from '../ui/Modal';

export function DocumentNavigationGuard({
  id,
  protectedChanges,
  saving,
}: {
  readonly id: string;
  readonly protectedChanges: boolean;
  readonly saving: boolean;
}) {
  const [open, setOpen] = useState(false);
  const pending = useRef<(() => void) | undefined>(undefined);
  useLayoutEffect(() => {
    if (!protectedChanges) return;
    const warn = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    const release = () => {
      pending.current = undefined;
      setOpen(false);
      window.removeEventListener('beforeunload', warn);
    };
    const unregister = registerNavigationGuard({
      release,
      leaves: ({ url, state }) => {
        if (pending.current) return true;
        const route = parseRoute(new URL(url, window.location.origin).pathname);
        const background =
          state && typeof state === 'object' && 'usageBackground' in state
            ? state.usageBackground
            : undefined;
        const destination =
          route.kind === 'usage' && typeof background === 'string'
            ? parseRoute(new URL(background, window.location.origin).pathname)
            : route;
        // The service shell retains its last Agent Manager page while showing the gateway.
        if (['event_gateway', 'bots', 'bot', 'bot_new'].includes(destination.kind)) return false;
        return destination.kind !== 'document' || destination.documentId !== id;
      },
      confirm: (proceed) => {
        if (pending.current) return;
        pending.current = proceed;
        setOpen(true);
      },
    });
    window.addEventListener('beforeunload', warn);
    return () => {
      unregister();
      release();
    };
  }, [id, protectedChanges]);
  const stay = () => {
    pending.current = undefined;
    setOpen(false);
  };
  return (
    <Modal
      open={open && protectedChanges}
      title="Leave without saving?"
      description={
        saving
          ? 'Your save has not been confirmed. Leaving will not cancel work already accepted by the service. Unsaved changes may be lost if you refresh or close this tab.'
          : 'Your changes have not been saved. They may be lost if you refresh or close this tab.'
      }
      onClose={stay}
      footer={
        <>
          <Button onClick={stay}>Stay</Button>
          <Button
            variant="primary"
            onClick={() => {
              const proceed = pending.current;
              stay();
              proceed?.();
            }}
          >
            Leave without saving
          </Button>
        </>
      }
    />
  );
}
