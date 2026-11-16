import {define} from './common.mjs';
import {poll, ownedIntent} from './enforcement.mjs';
import {triggerSettingScenario} from './trigger-setting.mjs';
import {triggerCommandScenario} from './trigger-command.mjs';

export const triggerSetting = define(47, 'browser', 2,
  ctx => triggerSettingScenario(ctx, {poll, ownedIntent}), ['admin-fixtures', 'browser']);
export const triggerPause = define(75, 'global', 2,
  ctx => triggerCommandScenario(ctx, 'pause', {poll, ownedIntent}));
export const triggerResume = define(76, 'global', 2,
  ctx => triggerCommandScenario(ctx, 'resume', {poll, ownedIntent}));
