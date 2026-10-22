import { useEffect, useRef } from 'react';

import { useI18n } from '../i18n';
import type { ProjectSkill } from '../lib/types';
import { Spinner } from './ui';

interface SkillPickerProps {
  id: string;
  skills: ProjectSkill[];
  index: number;
  loading: boolean;
  connected: boolean;
  filtered: boolean;
  remotePreparationPending?: boolean;
  onIndex: (index: number) => void;
  onSelect: (skill: ProjectSkill) => void;
  onBlur: (next: EventTarget | null) => void;
}

export function SkillPicker(props: SkillPickerProps) {
  const { t } = useI18n();
  const selected = useRef<HTMLOptionElement>(null);
  useEffect(() => {
    selected.current?.scrollIntoView({ block: 'nearest' });
  }, [props.index]);

  const status = !props.connected
    ? t('chat.skillsDisconnected')
    : props.loading
      ? t('chat.skillsLoading')
      : props.skills.length === 0
        ? t(
            props.remotePreparationPending
              ? 'chat.skillsAwaitingRemote'
              : props.filtered
                ? 'chat.skillsNoMatch'
                : 'chat.skillsEmpty',
          )
        : null;

  return (
    <div className="skill-picker">
      <div className="skill-picker-heading">{t('chat.skillsLabel')}</div>
      <select
        id={props.id}
        aria-label={t('chat.skillsLabel')}
        aria-busy={props.loading}
        className="skill-picker-options"
        style={{ height: `${String(Math.min(6, Math.max(1, props.skills.length)) * 38)}px` }}
        tabIndex={-1}
        size={Math.min(6, Math.max(2, props.skills.length))}
        value={String(props.index)}
        hidden={Boolean(status)}
        onBlur={(event) => props.onBlur(event.relatedTarget)}
        onChange={(event) => {
          const skill = props.skills[Number(event.currentTarget.value)];
          if (skill) props.onSelect(skill);
        }}
        onMouseDown={(event) => {
          if (event.target instanceof HTMLOptionElement) {
            event.preventDefault();
            const skill = props.skills[Number(event.target.value)];
            if (skill) props.onSelect(skill);
          }
        }}
        onMouseMove={(event) => {
          if (event.target instanceof HTMLOptionElement) props.onIndex(Number(event.target.value));
        }}
      >
        {!status &&
          props.skills.map((skill, index) => (
            <option
              id={`${props.id}-${String(index)}`}
              ref={index === props.index ? selected : undefined}
              key={skill.path}
              value={String(index)}
              aria-selected={index === props.index}
              className="skill-option"
              title={skill.description}
            >
              ${skill.name}
            </option>
          ))}
      </select>
      {!status && props.skills[props.index] && (
        <div className="skill-picker-details">
          <span className="skill-option-description">{props.skills[props.index]?.description}</span>
          <span className="skill-option-path">{props.skills[props.index]?.path}</span>
        </div>
      )}
      {status && (
        <output className="skill-picker-status">
          {props.loading && props.connected && <Spinner size={14} />}
          <span>{status}</span>
        </output>
      )}
      <div className="skill-picker-hint">{t('chat.skillsKeyboardHint')}</div>
    </div>
  );
}
