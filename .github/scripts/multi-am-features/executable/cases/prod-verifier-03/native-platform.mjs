import {define, acquire} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';
import {nativeTelegramCase} from './native-telegram.mjs';
import {nativeWhatsappCase} from './native-whatsapp.mjs';
import {nativeDiscordCase} from './native-discord.mjs';
import {inspectLarkSources} from './lark-source-inspection.mjs';

export const nativePlatformCases = [
  nativeTelegramCase,
  nativeDiscordCase,
  nativeWhatsappCase,
  // Native app intake and NyxID relay resource bindings have distinct authority.
  // The former generic resourceOperation/result flags did not execute the
  // public attachment/message APIs or prove selected-connection isolation.
  // Inspect real saved sources and compiled UI first. Resource execution and
  // finality remain incomplete, so a fixture oracle still cannot pass this row.
  define(43, 'eg', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
    await ctx.step('step_1', async () => {
      const lease = await acquire(ctx, 'native_lark_resources');
      await inspectLarkSources(ctx, lease);
    });
    for (const step of ['step_2', 'expected_result']) await ctx.step(step, async () => {
      throw new Blocked('native_lark_resource_public_journeys_incomplete');
    });
  }),
];
