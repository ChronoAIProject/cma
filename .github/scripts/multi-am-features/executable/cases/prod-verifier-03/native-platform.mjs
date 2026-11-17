import {define, acquire} from './helpers.mjs';
import {nativeTelegramCase} from './native-telegram.mjs';
import {nativeWhatsappCase} from './native-whatsapp.mjs';
import {nativeDiscordCase} from './native-discord.mjs';
import {inspectLarkSources} from './lark-source-inspection.mjs';
import {runLarkNativeIntake} from './lark-native-journey.mjs';
import {runLarkNativeDocuments, validateLarkDocumentFixture} from './lark-native-documents.mjs';
import {validateLarkRelayFixture, runLarkRelayResources, verifyLarkRelayResult} from './lark-relay-resources.mjs';

export const nativePlatformCases = [
  nativeTelegramCase,
  nativeDiscordCase,
  nativeWhatsappCase,
  // Native app intake and NyxID relay resource bindings have distinct authority.
  // The former generic resourceOperation/result flags did not execute the
  // public attachment/message APIs or prove selected-connection isolation.
  // Inspect real saved sources and compiled UI, then execute separate native
  // and selected relay journeys with actual public requests and finality reads.
  define(43, 'eg', ['browser', 'user-fixtures', 'eg_browser'], async ctx => {
    let lease, initial, relay;
    await ctx.step('step_1', async () => {
      lease = await acquire(ctx, 'native_lark_resources');
      initial = await inspectLarkSources(ctx, lease);
    });
    await ctx.step('step_2', async () => {
      validateLarkDocumentFixture(lease);
      validateLarkRelayFixture(lease, initial);
      const nativeTurns = await runLarkNativeIntake(ctx, lease, initial);
      const documentTurns = await runLarkNativeDocuments(ctx, lease, initial, nativeTurns);
      relay = await runLarkRelayResources(ctx, lease, initial, documentTurns);
    });
    await ctx.step('expected_result', async () => {
      await verifyLarkRelayResult(ctx, lease, initial, relay);
    });
  }),
];
