import type { HelpTopic } from '../routing/docsRoutes';
import { statusLabel } from '../vocabulary/status';

interface HelpScenario {
  readonly title: string;
  readonly state: string;
  readonly action: string;
  readonly completion: string;
}
interface TaskGuide {
  readonly title: string;
  readonly description: string;
  readonly scenarios: readonly HelpScenario[];
}

export const TASK_GUIDES: Record<HelpTopic, TaskGuide> = {
  'start-a-task': {
    title: 'Start a task',
    description: 'Describe the result you need, review your inputs, and follow progress.',
    scenarios: [
      {
        title: 'Write and review your task',
        state:
          'Home asks what your Agent should work on. You may also see task templates and an option to add a GitHub repository.',
        action:
          'Describe the result you need and the context the Agent should use. If you choose a template, fill its required fields and review the task preview. Check any repository and ref you added, then choose Start task when it is available.',
        completion:
          'Submitted task shows the request you sent. Preparation progress means setup is still running; the Agent conversation shows the work and its results. Starting a task does not mean the work has finished.',
      },
      {
        title: 'When starting is unavailable',
        state:
          'A message may explain missing input, unavailable task creation, a connection requirement, or full Sandbox capacity.',
        action:
          'Follow the message beside the form. Complete required input or the offered connection step. If New agent is offered instead, create an Agent and start its conversation. For full capacity, review Manage environments before starting new work.',
        completion:
          'The form confirms the requirement is satisfied and allows the action. If receipt of your task is unconfirmed, use Recover original task on that same request; do not create a replacement task.',
      },
    ],
  },
  'environments-and-capacity': {
    title: 'Environments and capacity',
    description: 'Understand Sandbox slots, deletion progress, and available Computer tools.',
    scenarios: [
      {
        title: 'Check what uses capacity',
        state:
          'Home shows how many Sandbox environment slots are used. This count can differ from the Agent list: a Sandbox can contain multiple Agents or no Agents.',
        action:
          'Choose Environments in the sidebar to review your Sandboxes and their available actions. Use an existing eligible environment when appropriate. If the observation is stale or unavailable, choose Refresh Sandbox capacity.',
        completion:
          'A fresh capacity observation shows the used slots and limit. An unavailable observation does not confirm that there is space for a new Sandbox.',
      },
      {
        title: 'Delete a task in a paused Sandbox',
        state: `A stopped or failed Agent can be deleted after its Sandbox returns to ${statusLabel('active')}.`,
        action: `From the task menu, choose Open Sandbox to resume. Choose Resume if the Sandbox offers it. Wait until its status is ${statusLabel('active')}, then return to the task and choose Delete.`,
        completion:
          'Task deletion removes this Agent and its Worktree. The separate Delete action on the Sandbox removes the environment and all its Workspaces and Agents after its own confirmation.',
      },
      {
        title: 'Wait for environment deletion',
        state:
          'An environment marked Deleting still occupies a Sandbox slot. Stopping or deleting an Agent does not confirm that its containing Sandbox has been deleted.',
        action:
          'Review the Sandbox and its contents before using its Delete action. Follow the confirmation shown there. While deletion is pending, use its displayed progress and recovery actions; opening this guide does not request deletion.',
        completion:
          'The slot becomes available when Sandbox deletion is confirmed and the refreshed capacity count reflects it. A pending deletion is not a free slot.',
      },
      {
        title: 'Understand Computer capabilities',
        state:
          'Computer may show that a live screen or file browsing is not enabled, unsupported, or temporarily unavailable. Screen and Files have separate access requirements.',
        action:
          'Use Enable browser only when offered. Use a check or retry action for a temporary problem when available. Reconnect cannot enable a capability that the service does not allow. You can return to an available conversation without enabling Computer tools.',
        completion:
          'An available Screen or Files view confirms that capability is ready to use. A missing live screen alone does not mean that your files or conversation have been lost.',
      },
    ],
  },
  'recover-and-continue': {
    title: 'Recover and continue',
    description: 'Check an uncertain result while keeping the original request.',
    scenarios: [
      {
        title: 'When receipt is unconfirmed',
        state:
          'You may see Awaiting confirmation, an unconfirmed creation, or a message that the task could not be confirmed. A lost connection does not prove the request failed.',
        action:
          'For a task, return to the original task or conversation and choose Recover original task when offered. For a document in Artifacts, choose Retry creation in the open creation dialog. If you left the dialog, choose Resume creation in Artifacts to reopen the saved request, then choose Retry creation. Keep the original request while its outcome is unknown.',
        completion:
          'A confirmed receipt, recorded progress, or the original message appearing in the conversation establishes what was accepted. If the result remains unknown, keep checking the original request. Help navigation does not retry or submit work.',
      },
      {
        title: 'When work is already accepted',
        state:
          'Preparation, reconnection, or an update may still be in progress. You may be able to read history while input or Computer tools are unavailable.',
        action:
          'Keep the original Agent and follow its displayed status. Use the offered status check or retry for that operation. For a confirmed failure, read its explanation and use only the recovery actions offered there.',
        completion:
          'The operation reports its result, and the original conversation shows its recorded messages and available controls. A receipt or restored connection alone does not mean the task completed.',
      },
    ],
  },
  'find-your-outputs': {
    title: 'Find your outputs',
    description: 'Tell generated content, registered files, saved documents, and downloads apart.',
    scenarios: [
      {
        title: 'Generated and registered are different',
        state:
          'An answer may contain generated text or mention a file path. Files from this task lists output references registered with that answer when available.',
        action:
          'Read the answer, check Files from this task, or use Computer → Files when available. A path mentioned in prose alone does not confirm that a file was registered or that you can download it.',
        completion:
          'A registered entry identifies the output and its observed availability. Registration alone does not guarantee a successful download: a live file may have changed, been removed, or become inaccessible.',
      },
      {
        title: 'Find a saved document',
        state:
          'Artifacts lists documents you own or that have been shared with you. A generated answer is not automatically a saved document in this library.',
        action:
          'Open Artifacts and use its search and ownership filters. Open the document you need. After editing, use the document’s Save action and check the save result before leaving.',
        completion:
          'The document confirms that your changes are saved. This is separate from downloading a copy to your device; an unsaved editor or an unconfirmed save is not evidence of a saved revision.',
      },
      {
        title: 'Download an available output',
        state:
          'Files from this task may say Ready to download, Verify on download, File changed, or Access unavailable. A document reference may point to an earlier revision.',
        action:
          'Use the offered download action and read its result. Retry an availability check when offered. If a file changed or access is unavailable, follow that status instead of assuming the original bytes are still accessible. A document revision download keeps the referenced revision when indicated.',
        completion:
          'Check that your browser’s download completed and open the downloaded file. A generated answer, registered reference, or saved document by itself does not prove that a download completed.',
      },
    ],
  },
};
