import type { ReactNode } from 'react';
import { Pill } from '../ui/Pill';
import type { SessionCreator, SessionService, SessionSkill } from './types';
import { useState } from 'react';

/**
 * The pieces the details drawer is built from — `docs/Web_Style.md` §17.
 *
 * Separated from the drawer itself because the drawer is now a composition
 * rather than a list, and a file that holds both the arrangement and every
 * thing being arranged is the file nobody can find anything in.
 *
 * One rule runs through all of it: **absent and empty are different answers**
 * and are drawn differently. A catalogue that could not be asked, a grant that
 * predates the record, and a session that was given nothing are three facts,
 * and collapsing them into a blank row answers a question nobody asked.
 */

/**
 * A titled band of facts.
 *
 * The title carries the rank — 11px, 600, tracked and in `--ink-3` — and the
 * values sit at reading size in `--ink`. That inversion is the whole point of
 * the redesign: the previous revision set labels and values two steps apart in
 * size and one in colour, which made a column of labels compete with the
 * answers under them.
 *
 * `count` is drawn beside the title where a section holds a list, so the
 * number is legible without counting the rows.
 */
export function Section({
  title,
  count,
  note,
  children,
}: {
  readonly title: string;
  readonly count?: number;
  /**
   * One line saying what the band is, for a title that names a thing without
   * saying what kind of thing it is.
   *
   * "Reaches" is the case it exists for: the band lists slugs, and nothing on
   * the surface said those were services on the identity platform rather than
   * something this product invented.
   */
  readonly note?: string;
  readonly children: ReactNode;
}) {
  return (
    <section className="facts__section">
      <h3 className="facts__title">
        <span>{title}</span>
        {count === undefined ? null : <span className="facts__count">{count}</span>}
      </h3>
      {note === undefined ? null : <p className="facts__subtitle">{note}</p>}
      {children}
    </section>
  );
}

/** A section's facts, as a two-column definition grid. */
export function Facts({ children }: { readonly children: ReactNode }) {
  return <dl className="facts__grid">{children}</dl>;
}

/**
 * One labelled fact.
 *
 * `tone` paints the value and is spent only where the colour carries a fact of
 * its own — an unrestricted agent, a failed state. Every other value is `--ink`,
 * because a panel that tints five rows for decoration has no way left to say
 * that the sixth one matters.
 */
export function Fact({
  label,
  tone,
  note,
  children,
}: {
  readonly label: string;
  readonly tone?: 'live' | 'wait' | 'fail';
  readonly note?: string;
  readonly children: ReactNode;
}) {
  return (
    <div className="facts__fact">
      <dt className="facts__label">{label}</dt>
      <dd className="facts__value">
        <span className="facts__answer" data-tone={tone}>
          {children}
        </span>
        {/*
         * The sentence under the answer, where the answer is a word somebody
         * did not choose and cannot look up from here. "Guarded" names a
         * setting; it does not say what it does.
         */}
        {note === undefined ? null : <span className="facts__note">{note}</span>}
      </dd>
    </div>
  );
}

/** A value the control plane did not report, said rather than left blank. */
export function Unknown({ children }: { readonly children: ReactNode }) {
  return <span className="facts__unknown">{children}</span>;
}

/**
 * What a session's credential may reach.
 *
 * Three answers drawn as three things. A list is what it was granted. An empty
 * list is a session that reaches nothing, said plainly. And a control plane
 * that did not report it at all says so, rather than being drawn as either.
 */
export function ReachList({ services }: { readonly services?: readonly SessionService[] }) {
  if (services === undefined) {
    return <Unknown>Not reported by the control plane</Unknown>;
  }
  if (services.length === 0) {
    return <Unknown>Nothing — this session reaches no services</Unknown>;
  }
  return (
    <ul className="facts__list">
      {services.map((service) => (
        <li className="facts__entry" key={service.id}>
          <span className="facts__entryhead">
            {/*
             * The name where the catalogue offered one, and the identifier
             * always: a catalogue that could not be asked costs the name and
             * not the answer, and an identifier alone is still true.
             */}
            <span className="facts__entryname">{service.name ?? service.id}</span>
            {/*
             * Mandated entries are drawn as fixed, matching the picker's
             * treatment and for its stated reason: a person deciding what a
             * session may touch should see the whole answer.
             */}
            {service.mandated ? <Pill>always</Pill> : null}
          </span>
          {service.name === undefined ? null : <span className="facts__entryid">{service.id}</span>}
        </li>
      ))}
    </ul>
  );
}

/**
 * The skills installed into the session's sandbox.
 *
 * The same three answers as reach, and deliberately the same shape: both are
 * "what was this session given", and drawing them differently would suggest
 * they are different kinds of fact.
 */
export function SkillList({ skills }: { readonly skills?: readonly SessionSkill[] }) {
  if (skills === undefined) {
    return <Unknown>Not recorded for this session</Unknown>;
  }
  if (skills.length === 0) {
    return <Unknown>None — this session was given no skills</Unknown>;
  }
  return (
    <ul className="facts__list">
      {skills.map((skill) => (
        <li className="facts__entry" key={skill.id}>
          <span className="facts__entryhead">
            <span className="facts__entryname">{skill.name ?? skill.id}</span>
          </span>
          {/*
           * The summary where the catalogue gave one, and the identifier where
           * it did not — never both, because at 360px the second line is the
           * only one there is and a description is the more useful of the two.
           */}
          {skill.description === undefined ? (
            skill.name === undefined ? null : (
              <span className="facts__entryid">{skill.id}</span>
            )
          ) : (
            <span className="facts__entrynote">{skill.description}</span>
          )}
        </li>
      ))}
    </ul>
  );
}

/**
 * The creator's picture, or their initial when there is none.
 *
 * A picture that fails to load falls back the same way rather than leaving a
 * broken image in a panel somebody is reading to identify a session.
 */
export function CreatorFace({ creator }: { readonly creator: SessionCreator }) {
  const [broken, setBroken] = useState(false);
  const initial = creator.displayName?.trim().charAt(0).toUpperCase();
  if (creator.avatarUrl === undefined || broken) {
    return (
      <span className="facts__face" aria-hidden="true">
        {initial === undefined || initial === '' ? '?' : initial}
      </span>
    );
  }
  return (
    <img
      className="facts__face"
      src={creator.avatarUrl}
      alt=""
      onError={() => {
        setBroken(true);
      }}
    />
  );
}
