import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import { requestBytes, type OperationOptions } from './http.js';
import { boolean, enumeration, integer, object, text, type Output } from './schema.js';

const capabilities = object({
  task_home_admission: boolean,
  suggested_followup_admission: boolean,
  desktop_enable: boolean,
  browser_takeover: boolean,
  replay_presentation: boolean,
});
const policy = object({
  object: enumeration('cma.ui.task_experience'),
  schema_version: integer,
  account_id: text,
  capabilities,
});
export type TaskExperienceCapabilities = Output<typeof capabilities>;
export type TaskExperience = Output<typeof policy>;
export interface TaskExperienceClient {
  read(options?: OperationOptions): Promise<TaskExperience>;
}
export class TaskExperiencePolicy implements TaskExperienceClient {
  constructor(private readonly config: ResolvedConfig) {}

  async read(options: OperationOptions = {}): Promise<TaskExperience> {
    const bytes = await requestBytes(
      this.config,
      '/api/v2/ui/task-experience',
      { ...options, expectedStatus: 200 },
      4096,
    );
    try {
      const value = policy(JSON.parse(new TextDecoder().decode(bytes)));
      if (value.schema_version !== 1 || !value.account_id)
        throw new ProtocolError('Unsupported task experience policy.');
      return value;
    } catch (cause) {
      throw new ProtocolError('Invalid task experience policy.', { cause });
    }
  }
}
