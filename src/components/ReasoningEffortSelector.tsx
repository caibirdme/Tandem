import * as Dropdown from '@radix-ui/react-dropdown-menu';
import { Check, ChevronDown, Lightbulb } from 'lucide-react';

import { useI18n } from '../i18n';
import { isReasoningEffort, reasoningEffortOptions } from '../lib/reasoningEffort';
import type { ReasoningEffort, ReasoningEffortValue } from '../lib/types';
import { Spinner } from './ui';

export function ReasoningEffortSelector({
  effort,
  defaultEffort,
  supportedEfforts,
  disabled,
  saving,
  uncertain,
  onChange,
}: {
  effort: ReasoningEffort;
  defaultEffort: ReasoningEffort;
  supportedEfforts?: ReasoningEffortValue[] | null | undefined;
  disabled: boolean;
  saving: boolean;
  uncertain: boolean;
  onChange: (effort: ReasoningEffort) => void;
}) {
  const { t } = useI18n();
  const defaultLabel =
    defaultEffort === null
      ? t('chat.reasoningDefault')
      : t('chat.reasoningDefaultValue', { effort: defaultEffort });
  const label = uncertain ? t('permissions.unconfirmed') : effort === null ? defaultLabel : effort;
  const locked = disabled || saving || uncertain;
  const options = reasoningEffortOptions(supportedEfforts);
  return (
    <Dropdown.Root>
      <Dropdown.Trigger
        type="button"
        className="model-selector reasoning-effort-selector"
        disabled={locked}
        aria-label={t('chat.reasoningLabel', { name: label })}
        aria-busy={saving}
        title={
          uncertain
            ? t('chat.reasoningUnconfirmedHint')
            : saving
              ? t('chat.reasoningSaving')
              : t('chat.reasoningHint')
        }
      >
        {saving ? <Spinner size={13} /> : <Lightbulb size={13} />}
        <span className="model-selector-label">
          {effort === null && !uncertain ? t('chat.reasoningDefault') : label}
        </span>
        <ChevronDown size={12} />
      </Dropdown.Trigger>
      <Dropdown.Portal>
        <Dropdown.Content
          className="dropdown reasoning-effort-menu"
          side="top"
          align="start"
          sideOffset={12}
        >
          <Dropdown.Label className="dropdown-label">{t('chat.reasoningTitle')}</Dropdown.Label>
          <Dropdown.RadioGroup
            value={effort ?? 'default'}
            onValueChange={(value) => {
              if (
                !locked &&
                (value === 'default' ||
                  (isReasoningEffort(value) && value !== null && options.includes(value)))
              )
                onChange(value === 'default' ? null : value);
            }}
          >
            {(['default', ...options] as const).map((value) => (
              <Dropdown.RadioItem
                key={value}
                value={value}
                className="dropdown-item"
                disabled={locked}
              >
                <span>{value === 'default' ? defaultLabel : value}</span>
                <Dropdown.ItemIndicator>
                  <Check size={14} />
                </Dropdown.ItemIndicator>
              </Dropdown.RadioItem>
            ))}
          </Dropdown.RadioGroup>
          <Dropdown.Label className="dropdown-label reasoning-effort-hint">
            {t('chat.reasoningHint')}
          </Dropdown.Label>
        </Dropdown.Content>
      </Dropdown.Portal>
    </Dropdown.Root>
  );
}
