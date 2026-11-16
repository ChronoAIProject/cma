import {browserCases} from './browser.mjs';
import {platformCases} from './platform.mjs';
import {toolCases} from './tools.mjs';
import {moreBrowserCases} from './browser-more.mjs';
import {channelUiCases} from './channel-ui.mjs';

import {githubCases} from './github.mjs';

import {mcpCases} from './mcp.mjs';

import {mediaToolCases} from './media-tools.mjs';

import {nativePlatformCases} from './native-platform.mjs';

import {botCases} from './bots.mjs';

import {bindingCases} from './bindings.mjs';
export {setups} from './setup.mjs';

import {interactionCases} from './interactions.mjs';

import {usageDetailCases} from './usage-details.mjs';

import {channelCases} from './channel-remaining.mjs';

import {connectionCases} from './connections.mjs';

import {consumerCreationCases} from './consumer-creation.mjs';

import {consoleCases} from './console.mjs';

export const cases = [...browserCases, ...moreBrowserCases, ...channelUiCases, ...platformCases, ...toolCases, ...githubCases, ...mcpCases, ...mediaToolCases, ...nativePlatformCases, ...botCases, ...bindingCases, ...interactionCases, ...usageDetailCases, ...channelCases, ...connectionCases, ...consumerCreationCases, ...consoleCases]
  .sort((a, b) => a.id.localeCompare(b.id));

// Parent-owned supplemental journey; never add it to the frozen C275 counts.
export {e1Journey} from './e1.mjs';
