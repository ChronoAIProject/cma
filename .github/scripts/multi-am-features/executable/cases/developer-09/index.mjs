import {cases as navigation} from './navigation.mjs';
import {cases as ranges} from './usage-ranges.mjs';
import {cases as inventory} from './api-inventory.mjs';
import {cases as fleet} from './fleet.mjs';
import {cases as pools} from './warm-pools.mjs';
export const cases = [...navigation, ...ranges, ...inventory, ...fleet, ...pools];
