import {define} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';
import {nativeTelegramCase} from './native-telegram.mjs';
import {nativeWhatsappCase} from './native-whatsapp.mjs';
import {nativeDiscordCase} from './native-discord.mjs';

export const nativePlatformCases = [
  nativeTelegramCase,
  nativeDiscordCase,
  nativeWhatsappCase,
  // Native app intake and NyxID relay resource bindings have distinct authority.
  // The former generic resourceOperation/result flags did not execute the
  // public attachment/message APIs or prove selected-connection isolation.
  // Keep every frozen step, but prevent a fixture oracle from passing this row
  // until the actual native and relay resource journeys are authored.
  define(43, 'eg', ['eg'], async ctx => {
    for (const step of ['step_1', 'step_2', 'expected_result']) await ctx.step(step, async () => {
      throw new Blocked('native_lark_resource_public_journeys_incomplete');
    });
  }),
];
