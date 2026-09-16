import type { BridgeConfiguration, BridgeConnectorChoices } from '@chronoai/cma-sdk';
import { Picker } from '../ui/Picker';
import { Input } from '../ui/Input';

export function ConnectionFields({
  value,
  choices,
  disabled,
  existing,
  onChange,
}: {
  readonly value: BridgeConfiguration;
  readonly choices?: BridgeConnectorChoices;
  readonly disabled: boolean;
  readonly existing: boolean;
  readonly onChange: (value: BridgeConfiguration) => void;
}) {
  const binding = value.binding;
  const bots = choices?.bots ?? [];
  const keys = choices?.keys ?? [];
  const routes = (choices?.routes ?? []).filter(
    (route) =>
      route.key_id === binding.key_id &&
      (route.bot_id === null || route.bot_id === binding.bot_id) &&
      (route.platform === binding.platform || route.platform === '*'),
  );
  return (
    <fieldset disabled={disabled} className="bridges__fields">
      <legend>Channel connection</legend>
      <label>
        Bridge name
        <Input
          value={value.display_name}
          maxLength={200}
          onChange={(event) => onChange({ ...value, display_name: event.target.value })}
        />
      </label>
      <label>
        Existing bot
        <Picker
          label="Existing channel bot"
          value={binding.bot_id}
          disabled={disabled || existing}
          options={[
            { value: '', label: 'Select a bot' },
            ...bots.map((bot) => ({
              value: bot.id,
              label: `${bot.platform} · ${bot.id}`,
              unavailable: bot.active ? undefined : 'Bot is inactive',
            })),
          ]}
          onChange={(id) => {
            const bot = bots.find((item) => item.id === id);
            if (bot !== undefined)
              onChange({
                ...value,
                binding: { ...binding, bot_id: id, platform: bot.platform, route_ids: [] },
              });
          }}
        />
      </label>
      {existing ? (
        <p>
          The selected bot and platform belong to this bridge. Use a new bridge to connect a
          different bot.
        </p>
      ) : null}
      <label>
        Existing Agent key
        <Picker
          label="Existing channel key"
          value={binding.key_id}
          disabled={disabled}
          options={[
            { value: '', label: 'Select a key' },
            ...keys.map((key) => ({
              value: key.id,
              label: key.id,
              unavailable: key.active ? undefined : 'Key is inactive',
            })),
          ]}
          onChange={(key_id) =>
            onChange({ ...value, binding: { ...binding, key_id, route_ids: [] } })
          }
        />
      </label>
      <div role="group" aria-label="Selected channel routes">
        <p>Matching routes</p>
        {routes.length === 0 ? (
          <p>No matching routes are available for this selection.</p>
        ) : (
          routes.map((route) => (
            <label className="bridges__check" key={route.id}>
              <input
                type="checkbox"
                disabled={disabled || !route.active}
                checked={binding.route_ids.includes(route.id)}
                onChange={(event) =>
                  onChange({
                    ...value,
                    binding: {
                      ...binding,
                      route_ids: event.target.checked
                        ? [...binding.route_ids, route.id]
                        : binding.route_ids.filter((id) => id !== route.id),
                    },
                  })
                }
              />
              {route.id}
              {route.active ? '' : ' · Inactive'}
            </label>
          ))
        )}
      </div>
      <p>
        Choose the matching bot, key and route after configuring this bridge’s callback on the
        dedicated key in NyxID.
      </p>
    </fieldset>
  );
}
