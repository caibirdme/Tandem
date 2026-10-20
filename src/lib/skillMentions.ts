import type { SelectedSkill } from './types';

export interface SkillQuery {
  start: number;
  end: number;
  prefix: string;
}

export interface SkillMention {
  start: number;
  end: number;
  skill: SelectedSkill;
}

export interface SkillDraft {
  text: string;
  mentions: SkillMention[];
}

export interface SkillInputEdit {
  text: string;
  start: number;
  end: number;
  inputType: string;
}

const nameCharacter = /[\p{L}\p{N}_.:-]/u;
const mentionBoundary = /[\s([{]/;

export function findSkillQuery(
  text: string,
  selectionStart: number,
  selectionEnd = selectionStart,
): SkillQuery | null {
  if (selectionStart !== selectionEnd || selectionStart < 0 || selectionEnd > text.length)
    return null;
  const match = /(?:^|[\s([{])\$([\p{L}_][\p{L}\p{N}_.:-]*|)$/u.exec(text.slice(0, selectionStart));
  if (!match) return null;
  const prefix = match[1] ?? '';
  const start = selectionStart - prefix.length - 1;
  let end = selectionStart;
  while (end < text.length && nameCharacter.test(text[end] ?? '')) end += 1;
  // A caret immediately before an amount is still a currency expression.
  if (!prefix && /^\p{N}/u.test(text.slice(selectionStart, end))) return null;
  return { start, end, prefix };
}

function validMention(text: string, mention: SkillMention): boolean {
  return (
    text.slice(mention.start, mention.end) === `$${mention.skill.name}` &&
    (mention.start === 0 || mentionBoundary.test(text[mention.start - 1] ?? '')) &&
    (mention.end === text.length || !nameCharacter.test(text[mention.end] ?? ''))
  );
}

function inputEditRange(
  before: string,
  after: string,
  input: SkillInputEdit | undefined,
): { start: number; end: number } | null {
  if (
    !input ||
    input.text !== before ||
    input.start < 0 ||
    input.end < input.start ||
    input.end > before.length
  )
    return null;
  let { start, end } = input;
  const offset = after.length - before.length;
  if (start === end && offset < 0) {
    if (input.inputType.endsWith('Backward')) start += offset;
    else if (input.inputType.endsWith('Forward')) end -= offset;
    else return null;
  }
  const newEnd = end + offset;
  if (
    start < 0 ||
    end > before.length ||
    newEnd < start ||
    before.slice(0, start) !== after.slice(0, start) ||
    before.slice(end) !== after.slice(newEnd)
  )
    return null;
  return { start, end };
}

function possibleEditRange(before: string, after: string): { start: number; end: number } {
  const length = Math.min(before.length, after.length);
  let prefix = 0;
  while (prefix < length && before[prefix] === after[prefix]) prefix += 1;
  let suffix = 0;
  while (suffix < length && before[before.length - suffix - 1] === after[after.length - suffix - 1])
    suffix += 1;
  // Repeated text can place the same edit at multiple offsets. Treat the entire
  // ambiguous region as edited rather than transferring an explicit selection.
  const overlap = Math.max(0, prefix + suffix - length);
  return { start: prefix - overlap, end: before.length - suffix + overlap };
}

export function editSkillDraft(
  draft: SkillDraft,
  text: string,
  input?: SkillInputEdit,
): SkillDraft {
  if (text === draft.text) return draft;
  const range = inputEditRange(draft.text, text, input) ?? possibleEditRange(draft.text, text);
  const offset = text.length - draft.text.length;
  const mentions = draft.mentions.flatMap((mention) => {
    const updated =
      mention.end <= range.start
        ? mention
        : mention.start >= range.end
          ? { ...mention, start: mention.start + offset, end: mention.end + offset }
          : null;
    return updated && validMention(text, updated) ? [updated] : [];
  });
  return { text, mentions };
}

export function selectSkill(
  draft: SkillDraft,
  query: SkillQuery,
  skill: SelectedSkill,
): { draft: SkillDraft; caret: number } {
  const mentionText = `$${skill.name}`;
  const suffix = draft.text.slice(query.end);
  const separator = suffix.startsWith(' ') ? '' : ' ';
  const text = `${draft.text.slice(0, query.start)}${mentionText}${separator}${suffix}`;
  const updated = editSkillDraft(draft, text, {
    text: draft.text,
    start: query.start,
    end: query.end,
    inputType: 'insertReplacementText',
  });
  const mention: SkillMention = {
    start: query.start,
    end: query.start + mentionText.length,
    skill: { name: skill.name, path: skill.path },
  };
  return {
    draft: {
      text,
      mentions: [
        ...updated.mentions.filter(
          (current) => current.end <= mention.start || current.start >= mention.end,
        ),
        mention,
      ].toSorted((left, right) => left.start - right.start),
    },
    caret: mention.end + 1,
  };
}

export function selectedSkills(draft: SkillDraft): SelectedSkill[] {
  const unique = new Map<string, SelectedSkill>();
  for (const mention of draft.mentions) {
    if (validMention(draft.text, mention))
      unique.set(mention.skill.path, { name: mention.skill.name, path: mention.skill.path });
  }
  return [...unique.values()];
}

export function restoreSkillDraft(text: string, skills: SelectedSkill[] = []): SkillDraft {
  const mentions: SkillMention[] = [];
  for (const skill of skills) {
    const token = `$${skill.name}`;
    const matches: SkillMention[] = [];
    let start = text.indexOf(token);
    while (start !== -1) {
      const mention = { start, end: start + token.length, skill };
      if (validMention(text, mention)) matches.push(mention);
      if (matches.length > 1) break;
      start = text.indexOf(token, start + token.length);
    }
    // Failed sends retain identities without their original ranges. Ambiguous
    // restored selections require reselection instead of guessing an occurrence.
    const [mention] = matches;
    if (
      matches.length === 1 &&
      mention &&
      !mentions.some((current) => current.start === mention.start)
    )
      mentions.push(mention);
  }
  return { text, mentions };
}

export function skillPickerKey(
  key: string,
  modifiers: { shiftKey: boolean; altKey: boolean; ctrlKey: boolean; metaKey: boolean },
  composing: boolean,
  open: boolean,
  count: number,
): 'previous' | 'next' | 'select' | 'dismiss' | null {
  if (composing || !open) return null;
  if (key === 'Escape') return 'dismiss';
  if (modifiers.shiftKey || modifiers.altKey || modifiers.ctrlKey || modifiers.metaKey) return null;
  if (count === 0) return key === 'Enter' ? 'dismiss' : null;
  if (key === 'ArrowUp') return 'previous';
  if (key === 'ArrowDown') return 'next';
  if (key === 'Enter' || key === 'Tab') return 'select';
  return null;
}
