export const restorePhases = [
  'checkpoint_verification',
  'runtime_provisioning',
  'credential_delivery',
  'home_restore',
  'conversation_reopen',
  'queued_message_delivery',
  'publish_active',
] as const;

export const pausePhases = [
  'capture_and_publish_checkpoint',
  'pause_agents',
  'teardown_runtime',
  'publish_paused',
] as const;

export const lifecycleStageCopy: Readonly<Record<string, { label: string; detail: string }>> = {
  checkpoint_verification: {
    label: 'Checking your saved sandbox',
    detail: 'Checking that your saved work is available to restore.',
  },
  runtime_provisioning: {
    label: 'Starting your sandbox',
    detail: 'Starting the environment that will run your workspaces and agents.',
  },
  credential_delivery: {
    label: 'Connecting your services',
    detail: 'Restoring secure access to your connected services.',
  },
  home_restore: {
    label: 'Restoring your files and installed software',
    detail:
      'Downloading, unpacking and checking your saved files. Large sandboxes can take several minutes. File-level progress is not reported yet.',
  },
  conversation_reopen: {
    label: 'Reopening your agents',
    detail: 'Reconnecting your agents to their existing conversations.',
  },
  queued_message_delivery: {
    label: 'Continuing pending messages',
    detail: 'Checking messages waiting for your agents to resume.',
  },
  publish_active: {
    label: 'Finishing sandbox preparation',
    detail: 'Confirming your sandbox and agents are ready.',
  },
  capture_and_publish_checkpoint: {
    label: 'Saving your files and conversations',
    detail: 'Saving your sandbox before its environment is stopped.',
  },
  pause_agents: {
    label: 'Pausing your agents',
    detail: "Preserving each agent's place in its conversation.",
  },
  teardown_runtime: {
    label: 'Stopping the sandbox environment',
    detail: 'Releasing the environment after your work has been saved.',
  },
  publish_paused: {
    label: 'Finishing pause',
    detail: 'Confirming your sandbox is paused.',
  },
};

export const sandboxProgressCopy = {
  unknown: 'Preparing your sandbox',
  unknownDetail: 'Waiting for the next recorded preparation stage.',
  activity: 'Activity details',
  disconnected: 'Status updates interrupted. Reconnecting; the last confirmed stage is shown.',
  unavailable: 'Sandbox progress is unavailable. Reconnecting.',
  checking: 'Checking sandbox progress',
  background: 'Preparation continues if you close this page.',
  elapsed: 'Elapsed',
  checked: 'Last checked',
  started: 'Started',
} as const;
