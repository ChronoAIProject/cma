import {define, acquire} from './helpers.mjs';
import {Blocked} from '../../runner.mjs';
import {nativeTelegramCase} from './native-telegram.mjs';
import {nativeWhatsappCase} from './native-whatsapp.mjs';
import {nativeDiscordCase} from './native-discord.mjs';
import {inspectLarkSources} from './lark-source-inspection.mjs';
import {runLarkNativeIntake} from './lark-native-journey.mjs';

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
    let lease, initial;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'native_lark_resources');
      initial = await inspectLarkSources(ctx, lease);
    });
    await ctx.step('step_2', async () => {
      await runLarkNativeIntake(ctx, lease, initial);
      throw new Blocked('native_lark_resource_public_journeys_incomplete');
    });
    await ctx.step('expected_result', async () => {
      throw new Blocked('native_lark_resource_public_journeys_incomplete');
    });
  }),
];
