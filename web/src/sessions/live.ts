import type { Session } from './types';
import type { TerminalDriver } from '../terminal/types';

/**
 * A session paired with the one driver that carries it.
 *
 * The pairing exists so a view never has to look a driver up and never has to
 * cope with a session that has none. It also fixes driver identity for as long
 * as the session's state calls for the same kind of driver, which the terminal
 * surface depends on: a driver that changed identity between renders would
 * rebuild the terminal and drop its scrollback.
 *
 * Constructing a driver is not the same as connecting one. A driver opens its
 * transport when a surface attaches it, so the console may hold a pairing for
 * every session while attaching only the handful it can afford.
 */
export interface LiveSession {
  readonly session: Session;
  /**
   * Null when the session has nothing to read yet — it is still starting, or
   * it is being moved onto a newer runtime. Neither has a screen to show and
   * neither accepts input, so a node renders the reason instead of a terminal.
   */
  readonly driver: TerminalDriver | null;
}
