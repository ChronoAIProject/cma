import type { NavigationScopeChoices } from './NavigationScope';
import { Modal } from '../ui/Modal';
import { Button } from '../ui/Button';
import { IconWarningOutline16, IconRefreshOutline16 } from '../ui/icons';
import './AccessStatusPanel.css';

export function AccessStatusPanel({
  choices,
  scope,
  onClose,
}: {
  readonly choices: NavigationScopeChoices;
  readonly scope: string;
  readonly onClose: () => void;
}) {
  const selected =
    choices.options.find((option) => option.value === scope)?.label ?? 'Selected organization';
  return (
    <Modal
      open
      title="Access"
      description="Your current scope and organization choices."
      onClose={onClose}
      footer={<Button onClick={onClose}>Back to your current view</Button>}
    >
      <div className="access-status">
        <span className="access-status__eyebrow">Current scope · unchanged</span>
        <h3>{selected}</h3>
        <div className="access-status__notice" role={choices.unavailable ? 'alert' : 'status'}>
          <IconWarningOutline16 />
          <div>
            <strong>
              {choices.loading
                ? 'Checking your organizations…'
                : choices.unavailable
                  ? 'Your organizations didn’t load'
                  : 'Organization choices are available'}
            </strong>
            <p>
              {choices.unavailable
                ? 'This does not mean you lost access. Your current scope and draft stay as they are.'
                : choices.loading
                  ? 'Your current scope and draft stay as they are.'
                  : 'You can return to your current view or choose a scope in the sidebar.'}
            </p>
          </div>
          {choices.reload ? (
            <Button
              variant="outline"
              icon={<IconRefreshOutline16 />}
              disabled={choices.loading}
              onClick={choices.reload}
            >
              Try again
            </Button>
          ) : null}
        </div>
      </div>
    </Modal>
  );
}
