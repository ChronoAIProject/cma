import './Settings.css';
import type { AgentSettingsView } from '../api/client';
import { Input } from '../ui/Input';

export const REQUIRED_AGENT_SCOPES = ['chrono-llm-public', 'ornn-api'] as const;
const EFFORT_CHOICES = ['minimal', 'low', 'medium', 'high', 'x_high'] as const;
export function SettingsFields({
  value,
  onChange,
  disabled,
}: {
  readonly value: AgentSettingsView;
  readonly onChange: (value: AgentSettingsView) => void;
  readonly disabled: boolean;
}) {
  return (
    <div className="agent-settings-fields">
      <label>
        Model
        <Input
          aria-label="Model"
          value={value.model}
          disabled={disabled}
          list="agent-model-options"
          onChange={(event) => onChange({ ...value, model: event.target.value })}
          maxLength={128}
        />
        <datalist id="agent-model-options">
          <option value="gpt-6" />
        </datalist>
      </label>
      <label>
        Reasoning effort
        <select
          value={value.reasoning_effort}
          disabled={disabled}
          onChange={(event) =>
            onChange({
              ...value,
              reasoning_effort: event.target.value as AgentSettingsView['reasoning_effort'],
            })
          }
        >
          {EFFORT_CHOICES.map((effort) => (
            <option key={effort} value={effort}>
              {effort === 'x_high' ? 'Very high' : effort.charAt(0).toUpperCase() + effort.slice(1)}
            </option>
          ))}
        </select>
      </label>
      <label>
        Permission mode
        <select
          value={value.permission_mode}
          disabled={disabled}
          onChange={(event) =>
            onChange({
              ...value,
              permission_mode: event.target.value as AgentSettingsView['permission_mode'],
            })
          }
        >
          <option value="autonomous">YOLO (autonomous)</option>
          <option value="guarded">Guarded</option>
        </select>
      </label>
    </div>
  );
}
