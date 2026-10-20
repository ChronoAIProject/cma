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
  compact = false,
}: {
  readonly value: AgentSettingsView;
  readonly onChange: (value: AgentSettingsView) => void;
  readonly disabled: boolean;
  readonly models: readonly AgentModelOption[];
  readonly modelsLoading: boolean;
  readonly compact?: boolean;
}) {
  return (
    <div className={`agent-settings-fields${compact ? ' agent-settings-fields--compact' : ''}`}>
      <label>
        <span className={compact ? 'sr-only' : undefined}>Model</span>
        <Picker
          size={compact ? 'sm' : undefined}
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
            ...models.map((model) => ({
              value: model.id,
              label: model.label,
              unavailable: model.available ? undefined : 'This Agent cannot select this model yet.',
            })),
          ]}
          onChange={(model) => onChange({ ...value, model })}
        />
      </label>
      <label>
        <span className={compact ? 'sr-only' : undefined}>Reasoning effort</span>
        <Picker
          size={compact ? 'sm' : undefined}
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
        <span className={compact ? 'sr-only' : undefined}>Permission mode</span>
        <Picker
          size={compact ? 'sm' : undefined}
          placement="agent-settings-fields__picker"
          label="Permission mode"
          value={value.permission_mode}
          disabled={disabled}
          options={[
            { value: 'autonomous', label: compact ? 'YOLO' : 'YOLO (autonomous)' },
            { value: 'guarded', label: 'Guarded' },
          ]}
          onChange={(permission_mode) => onChange({ ...value, permission_mode })}
        />
      </label>
    </div>
  );
}
