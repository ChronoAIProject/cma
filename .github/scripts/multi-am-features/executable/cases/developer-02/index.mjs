import {cases as navigation} from './navigation.mjs';
import {cases as history} from './history.mjs';
import {cases as lifecycle} from './lifecycle.mjs';
export {setups} from './setups.mjs';
export {browserFromLease} from './browser-lease.mjs';
export const cases = [...navigation,...history,...lifecycle];
