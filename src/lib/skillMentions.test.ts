import { describe, expect, it } from 'vitest';

import {
  editSkillDraft,
  findSkillQuery,
  restoreSkillDraft,
  selectedSkills,
  selectSkill,
  skillPickerKey,
} from './skillMentions';
import type { SkillDraft } from './skillMentions';

const review = { name: 'review', path: '/project/.agents/skills/review/SKILL.md' };
const refactor = { name: 'refactor', path: '/project/.agents/skills/refactor/SKILL.md' };

function choose(text: string, skill = review): SkillDraft {
  const query = findSkillQuery(text, text.length);
  if (!query) throw new Error('Expected a skill query in test input');
  return selectSkill({ text, mentions: [] }, query, skill).draft;
}

describe('skill mention queries', () => {
  it.each([
    ['$', ''],
    ['$rev', 'rev'],
    ['Please use $rev', 'rev'],
    ['First line\n$review', 'review'],
    ['($review', 'review'],
    ['$custom_skill', 'custom_skill'],
    ['$レビュー', 'レビュー'],
  ])('opens and filters a skill query in %j', (text, prefix) => {
    expect(findSkillQuery(text, text.length)).toEqual({
      start: text.lastIndexOf('$'),
      end: text.length,
      prefix,
    });
  });

  it.each(['cost$review', '\\$review', '$10', '$3.50', '$$review', '$review ', '$review/notes'])(
    'leaves ordinary text %j alone',
    (text) => {
      expect(findSkillQuery(text, text.length)).toBeNull();
    },
  );

  it('replaces the entire token when the caret is inside it and preserves following text', () => {
    const text = 'Use $review next';
    const query = findSkillQuery(text, 7);
    expect(query).toEqual({ start: 4, end: 11, prefix: 're' });
    if (!query) throw new Error('Expected a skill query');
    expect(selectSkill({ text, mentions: [] }, query, refactor)).toEqual({
      draft: {
        text: 'Use $refactor next',
        mentions: [{ start: 4, end: 13, skill: refactor }],
      },
      caret: 14,
    });
  });

  it('does not open over a text selection or immediately before a price', () => {
    expect(findSkillQuery('$review', 1, 4)).toBeNull();
    expect(findSkillQuery('$100', 1)).toBeNull();
  });
});

describe('explicit skill selection', () => {
  it('attaches identity only after selecting a catalog entry', () => {
    expect(selectedSkills({ text: '$review ', mentions: [] })).toEqual([]);
    const draft = choose('$rev');
    expect(draft.text).toBe('$review ');
    expect(selectedSkills(draft)).toEqual([review]);
  });

  it.each(['$reviews ', '$rev ', '\\$review ', 'cost$review ', ''])(
    'removes selected identity when the mention becomes %j',
    (text) => {
      expect(selectedSkills(editSkillDraft(choose('$'), text))).toEqual([]);
    },
  );

  it('keeps a selected identity through edits before and after its mention', () => {
    let draft = choose('$rev');
    draft = editSkillDraft(draft, 'Please $review ');
    draft = editSkillDraft(draft, 'Please $review this file.');
    expect(selectedSkills(draft)).toEqual([review]);
    expect(draft.mentions[0]?.start).toBe(7);
  });

  it('does not reattach an identity when an edited name is restored by typing', () => {
    let draft = editSkillDraft(choose('$'), '$reviews ');
    draft = editSkillDraft(draft, '$review ');
    expect(selectedSkills(draft)).toEqual([]);
  });

  it('deduplicates selected identities without confusing similarly named skills', () => {
    let draft = choose('$');
    draft = editSkillDraft(draft, '$review $');
    const query = findSkillQuery(draft.text, draft.text.length);
    if (!query) throw new Error('Expected a second skill query');
    draft = selectSkill(draft, query, review).draft;
    expect(draft.mentions).toHaveLength(2);
    expect(selectedSkills(draft)).toEqual([review]);
    draft = editSkillDraft(draft, '$review ', {
      text: draft.text,
      start: 8,
      end: 16,
      inputType: 'deleteContentBackward',
    });
    expect(selectedSkills(draft)).toEqual([review]);
  });

  it('does not transfer a deleted selection to an identical unselected mention', () => {
    const draft = {
      text: '$review $review ',
      mentions: [{ start: 0, end: 7, skill: review }],
    };
    const changed = editSkillDraft(draft, '$review ', {
      text: draft.text,
      start: 0,
      end: 8,
      inputType: 'deleteByCut',
    });
    expect(selectedSkills(changed)).toEqual([]);
  });

  it('keeps the surviving selected duplicate when text before it is deleted', () => {
    const draft = {
      text: '$review $review ',
      mentions: [{ start: 8, end: 15, skill: review }],
    };
    const changed = editSkillDraft(draft, '$review ', {
      text: draft.text,
      start: 0,
      end: 8,
      inputType: 'deleteContentForward',
    });
    expect(selectedSkills(changed)).toEqual([review]);
    expect(changed.mentions[0]?.start).toBe(0);
  });

  it('does not move a selection onto an identical pasted mention', () => {
    const draft = choose('$');
    const changed = editSkillDraft(draft, '$review $review ', {
      text: draft.text,
      start: 0,
      end: 0,
      inputType: 'insertFromPaste',
    });
    expect(changed.mentions).toEqual([{ start: 8, end: 15, skill: review }]);
  });

  it.each([0, 8])(
    'drops ambiguous duplicate identity at offset %i when no actual edit range is available',
    (start) => {
      const draft = {
        text: '$review $review ',
        mentions: [{ start, end: start + 7, skill: review }],
      };
      expect(selectedSkills(editSkillDraft(draft, '$review '))).toEqual([]);
      expect(
        selectedSkills(
          editSkillDraft(draft, '$review ', {
            text: 'an outdated input snapshot',
            start: 0,
            end: 8,
            inputType: 'deleteByCut',
          }),
        ),
      ).toEqual([]);
    },
  );

  it.each([
    { start: 8, end: 8, inputType: 'deleteContentBackward' },
    { start: 7, end: 7, inputType: 'deleteContentForward' },
  ])('preserves a selected token when only its trailing space is deleted: $inputType', (input) => {
    const draft = choose('$');
    expect(
      selectedSkills(editSkillDraft(draft, '$review', { ...input, text: draft.text })),
    ).toEqual([review]);
  });

  it('preserves distinct paths for duplicate names chosen from the catalog', () => {
    const otherReview = { ...review, path: '/project/.github/skills/review/SKILL.md' };
    let draft = choose('$');
    draft = editSkillDraft(draft, '$review $');
    const query = findSkillQuery(draft.text, draft.text.length);
    if (!query) throw new Error('Expected a second skill query');
    draft = selectSkill(draft, query, otherReview).draft;
    expect(selectedSkills(draft)).toEqual([review, otherReview]);
  });

  it('restores only exact surviving mentions from a failed send', () => {
    expect(selectedSkills(restoreSkillDraft('Use $review today', [review, refactor]))).toEqual([
      review,
    ]);
    expect(selectedSkills(restoreSkillDraft('Use $reviews today', [review]))).toEqual([]);
    expect(selectedSkills(restoreSkillDraft('Use \\$review today', [review]))).toEqual([]);
  });

  it('requires reselection for ambiguous restored mentions without changing the draft', () => {
    const text = '$review $review ';
    expect(restoreSkillDraft(text, [review])).toEqual({ text, mentions: [] });
    const mixed = restoreSkillDraft(`${text}$refactor`, [review, refactor]);
    expect(mixed.text).toBe(`${text}$refactor`);
    expect(selectedSkills(mixed)).toEqual([refactor]);
  });

  it('does not count escaped or incomplete names as ambiguous restored mentions', () => {
    const draft = restoreSkillDraft('\\$review $reviews $review ', [review]);
    expect(selectedSkills(draft)).toEqual([review]);
    expect(draft.mentions[0]?.start).toBe(18);
  });
});

describe('skill picker keyboard behavior', () => {
  const plain = { shiftKey: false, altKey: false, ctrlKey: false, metaKey: false };
  it.each([
    ['ArrowUp', 'previous'],
    ['ArrowDown', 'next'],
    ['Enter', 'select'],
    ['Tab', 'select'],
    ['Escape', 'dismiss'],
    ['a', null],
  ] as const)('maps %s while the picker has results', (key, result) => {
    expect(skillPickerKey(key, plain, false, true, 2)).toBe(result);
  });

  it('preserves multiline editing, IME confirmation, and closed-picker behavior', () => {
    expect(skillPickerKey('Enter', { ...plain, shiftKey: true }, false, true, 2)).toBeNull();
    expect(skillPickerKey('Enter', plain, true, true, 2)).toBeNull();
    expect(skillPickerKey('Enter', plain, false, false, 2)).toBeNull();
    expect(skillPickerKey('Tab', { ...plain, shiftKey: true }, false, true, 2)).toBeNull();
    expect(skillPickerKey('ArrowUp', { ...plain, ctrlKey: true }, false, true, 2)).toBeNull();
  });

  it('dismisses instead of sending when Enter is pressed on an empty or loading picker', () => {
    expect(skillPickerKey('Enter', plain, false, true, 0)).toBe('dismiss');
    expect(skillPickerKey('Tab', plain, false, true, 0)).toBeNull();
    expect(skillPickerKey('Escape', plain, false, true, 0)).toBe('dismiss');
  });
});
