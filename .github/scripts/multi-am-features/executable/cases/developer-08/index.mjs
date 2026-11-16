export {setups} from './setups.mjs';
import {editCase, conflictCase} from './edit.mjs';
import {browseCase, createCase} from './registry.mjs';
import {commentsReadCase, commentCase} from './comments.mjs';
import {peopleCase, linkCase} from './sharing.mjs';
export const cases = [browseCase, createCase, editCase, conflictCase, commentsReadCase, commentCase, peopleCase, linkCase];
