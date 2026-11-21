import { createContext } from 'react';

/**
 * The colour of the Sandbox a surface shows (Web_Style.md §28). Every character below it wears
 * this colour in the brand tone unless given its own, so one Agent's page needs no colour
 * threaded through each component that happens to draw the character.
 */
export const CharacterColorContext = createContext<string | undefined>(undefined);
