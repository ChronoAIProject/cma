import './Settings.css';
import type { AgentModelOption, AgentSettingsView } from '../api/client';
import { Picker } from '../ui/Picker';

export const REQUIRED_AGENT_SCOPES = ['chrono-llm-public', 'ornn-api'] as const;
const EFFORT_CHOICES = ['minimal', 'low', 'medium', 'high', 'x_high'] as const;
export function SettingsFields({
  value,
  onChange,
  disabled,
  models,
  modelsLoading,
}: {
  readonly value: AgentSettingsView;
  readonly onChange: (value: AgentSettingsView) => void;
  readonly disabled: boolean;
  readonly models: readonly AgentModelOption[];
  readonly modelsLoading: boolean;
}) {
  return (
    <div className="agent-settings-fields">
      <label>
        Model
        <Picker
          placement="agent-settings-fields__picker"
          label="Model"
          value={value.model}
          disabled={disabled || modelsLoading || models.length === 0}
          options={[
            ...(models.some((model) => model.id === value.model)
              ? []
              : [
                  {
                    value: value.model,
                    label: value.model,
                    unavailable: 'Current model is not in the available catalog.',
                  },
                ]),
            ...models.map((model) => ({ value: model.id, label: model.label })),
          ]}
          onChange={(model) => onChange({ ...value, model })}
        />
      </label>
      <label>
        Reasoning effort
        <Picker
          placement="agent-settings-fields__picker"
          label="Reasoning effort"
          value={value.reasoning_effort}
          disabled={disabled}
          options={EFFORT_CHOICES.map((effort) => ({
            value: effort,
            label:
              effort === 'x_high' ? 'Very high' : effort.charAt(0).toUpperCase() + effort.slice(1),
          }))}
          onChange={(reasoning_effort) => onChange({ ...value, reasoning_effort })}
        />
      </label>
      <label>
        Permission mode
        <Picker
          placement="agent-settings-fields__picker"
          label="Permission mode"
          value={value.permission_mode}
          disabled={disabled}
          options={[
            { value: 'autonomous', label: 'YOLO (autonomous)' },
            { value: 'guarded', label: 'Guarded' },
          ]}
          onChange={(permission_mode) => onChange({ ...value, permission_mode })}
        />
      </label>
    </div>
  );
}
