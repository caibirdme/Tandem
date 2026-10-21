import { Sparkles } from 'lucide-react';
import type { ReactNode } from 'react';

import { findSkillQuery } from '../lib/skillMentions';
import type { SelectedSkill } from '../lib/types';

import './MessageContent.css';

interface SentMessageContentProps {
  content: string;
  selectedSkills?: SelectedSkill[];
}

interface SentSkillMention {
  start: number;
  end: number;
  name: string;
}

const noSkills: SelectedSkill[] = [];

function sentSkillMentions(content: string, skills: SelectedSkill[]): SentSkillMention[] {
  const mentions: SentSkillMention[] = [];
  const names = new Set(skills.map((skill) => skill.name).filter(Boolean));

  for (const name of names) {
    const token = `$${name}`;
    let start = content.indexOf(token);
    while (start !== -1) {
      const end = start + token.length;
      const query = findSkillQuery(content, end);
      if (query?.start === start && query.end === end && query.prefix === name)
        mentions.push({ start, end, name });
      start = content.indexOf(token, end);
    }
  }

  const ordered = mentions.toSorted(
    (left, right) => left.start - right.start || right.end - left.end,
  );
  const separate: SentSkillMention[] = [];
  let end = 0;
  for (const mention of ordered) {
    if (mention.start >= end) {
      separate.push(mention);
      end = mention.end;
    }
  }
  return separate;
}

export function SentMessageContent({
  content,
  selectedSkills = noSkills,
}: SentMessageContentProps) {
  const parts: ReactNode[] = [];
  let cursor = 0;
  for (const mention of sentSkillMentions(content, selectedSkills)) {
    if (mention.start > cursor) parts.push(content.slice(cursor, mention.start));
    parts.push(
      <span className="sent-skill-mention" key={`skill-${String(mention.start)}-${mention.name}`}>
        <Sparkles size={13} strokeWidth={1.7} aria-hidden="true" />
        <span className="sent-skill-mention-name">${mention.name}</span>
      </span>,
    );
    cursor = mention.end;
  }
  if (cursor < content.length) parts.push(content.slice(cursor));
  return <p className="sent-message-content">{parts}</p>;
}
