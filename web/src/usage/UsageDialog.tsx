import type { UsageClient } from './client';
import { Modal } from '../ui/Modal';
import { MetricsDashboard } from './metrics/MetricsDashboard';
import { useMeasuredUsage } from './metrics/useMeasuredUsage';
import { useUsageSelection } from './metrics/selection';
import { UsageAgentPicker } from './UsageAgentPicker';
import { anonymousReportOwner, type UsageReportOwner } from './metrics/reportOwner';

export function UsageDialog({
  client,
  open,
  onClose,
  principal = anonymousReportOwner,
  appearanceNotice = false,
}: {
  readonly client?: UsageClient;
  readonly open: boolean;
  readonly onClose: () => void;
  readonly principal?: UsageReportOwner;
  readonly appearanceNotice?: boolean;
}) {
  const measured = useMeasuredUsage(client?.readMeasuredUsage, open);
  const [selection, onSelection] = useUsageSelection(false);
  return (
    <Modal keepMounted open={open} title="Your usage" onClose={onClose} placement="usage-dialog">
      <p className="usage-section-description">Usage only, not a bill</p>
      {appearanceNotice && <p role="status">Appearance is now in your account menu.</p>}
      {client ? (
        <MetricsDashboard
          api={client.usageMetrics}
          history={client.usage}
          admin={false}
          principal={principal}
          measured={measured}
          active={open}
          selection={selection}
          onSelection={onSelection}
          agentPicker={(draft, update) => (
            <UsageAgentPicker
              client={client}
              sandbox={draft.sandbox}
              value={draft.agent}
              active={open}
              onChange={(agent) => update({ ...draft, agent })}
            />
          )}
        />
      ) : (
        <p role="status">
          Usage reporting is unavailable. Try again when the service is connected.
        </p>
      )}
    </Modal>
  );
}
