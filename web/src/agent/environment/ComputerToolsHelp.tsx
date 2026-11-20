import { useState } from 'react';
import { Button } from '../../ui/Button';
import { Modal } from '../../ui/Modal';

export function ComputerToolsHelp() {
  const [open, setOpen] = useState(false);
  return (
    <span
      onKeyDown={(event) => {
        if (open && event.key === 'Escape') {
          event.preventDefault();
          event.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <Button size="sm" onClick={() => setOpen(true)}>
        About Computer tools
      </Button>
      <Modal open={open} title="About Computer tools" onClose={() => setOpen(false)}>
        <h3>Screen</h3>
        <p>
          A live screen is available only when the environment supports it and your account has
          access. If Enable browser is offered, you can choose it to start the live screen.
          Reconnect can recover a lost connection, but cannot enable a tool that this service does
          not allow.
        </p>
        <h3>Files</h3>
        <p>
          File browsing shows files that your account can access in this environment. Screen and
          file access are separate capabilities. A missing live screen does not mean that your files
          or conversation have been lost.
        </p>
        <p>
          When the conversation is available, you can return to it without enabling Computer tools.
          Temporary availability problems offer a check or retry action when it can help.
        </p>
      </Modal>
    </span>
  );
}
