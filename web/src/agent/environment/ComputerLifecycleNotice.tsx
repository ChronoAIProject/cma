import type { SandboxResourceAvailability } from '../sandboxInput';
import { Button } from '../../ui/Button';

export function ComputerLifecycleNotice({
  availability,
  onOpenSandbox,
}: {
  readonly availability: Exclude<SandboxResourceAvailability, { state: 'live' }>;
  readonly onOpenSandbox?: () => void;
}) {
  return (
    <>
      <p role="status">{availability.notice}</p>
      {availability.state === 'paused' && onOpenSandbox ? (
        <Button size="sm" onClick={onOpenSandbox}>
          Open Sandbox to resume
        </Button>
      ) : null}
    </>
  );
}
