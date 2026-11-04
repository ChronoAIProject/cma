import { createClient } from '@chronoai/cma-sdk';

export function taskExperience(accountId: string, enabled = true) {
  return createClient({
    fetch: async (path) => {
      if (!String(path).endsWith('/api/v2/ui/task-experience'))
        throw new Error('Unexpected policy request');
      return Response.json({
        object: 'cma.ui.task_experience',
        schema_version: 1,
        account_id: accountId,
        capabilities: {
          task_home_admission: enabled,
          suggested_followup_admission: enabled,
          desktop_enable: enabled,
          browser_takeover: enabled,
          replay_presentation: enabled,
        },
      });
    },
  }).taskExperience;
}
