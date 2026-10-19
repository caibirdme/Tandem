import * as Dropdown from '@radix-ui/react-dropdown-menu';
import { Check, ChevronDown, ShieldAlert, ShieldCheck } from 'lucide-react';

import { useI18n } from '../i18n';
import type { PermissionMode } from '../lib/types';
import { Spinner } from './ui';

export function PermissionModeSelector({
  mode,
  disabled,
  saving,
  uncertain,
  onChange,
}: {
  mode: PermissionMode;
  disabled: boolean;
  saving: boolean;
  uncertain: boolean;
  onChange: (mode: PermissionMode) => void;
}) {
  const { t } = useI18n();
  const modes = [
    { value: 'ask', label: t('permissions.ask'), description: t('permissions.askDescription') },
    {
      value: 'allow-all',
      label: t('permissions.allowAll'),
      description: t('permissions.allowAllDescription'),
    },
  ] as const;
  const authorized = mode === 'allow-all';
  const label = uncertain
    ? t('permissions.unconfirmed')
    : authorized
      ? t('permissions.allowAll')
      : t('permissions.ask');
  return (
    <Dropdown.Root>
      <Dropdown.Trigger
        type="button"
        className={`permission-mode-selector ${authorized ? 'is-authorized' : ''}`}
        disabled={disabled || saving || uncertain}
        aria-label={t('permissions.label', { name: label })}
        aria-busy={saving}
        title={
          uncertain
            ? t('permissions.confirmFirst')
            : saving
              ? t('permissions.saving')
              : disabled
                ? t('permissions.disabledHint')
                : t('permissions.change')
        }
      >
        {saving ? (
          <Spinner size={13} />
        ) : authorized ? (
          <ShieldAlert size={13} />
        ) : (
          <ShieldCheck size={13} />
        )}
        {label}
        <ChevronDown size={12} />
      </Dropdown.Trigger>
      <Dropdown.Portal>
        <Dropdown.Content
          className="dropdown permission-mode-menu"
          side="top"
          align="end"
          sideOffset={12}
        >
          <Dropdown.Label className="dropdown-label">
            {t('permissions.workPermissions')}
          </Dropdown.Label>
          <Dropdown.RadioGroup
            value={mode}
            onValueChange={(value) => {
              if (!disabled && !saving && !uncertain && (value === 'ask' || value === 'allow-all'))
                onChange(value);
            }}
          >
            {modes.map((item) => (
              <Dropdown.RadioItem
                key={item.value}
                value={item.value}
                className="dropdown-item permission-mode-option"
                disabled={disabled || saving || uncertain}
                textValue={item.label}
              >
                <span>
                  <strong>{item.label}</strong>
                  <small>{item.description}</small>
                </span>
                <Dropdown.ItemIndicator className="permission-mode-check">
                  <Check size={14} />
                </Dropdown.ItemIndicator>
              </Dropdown.RadioItem>
            ))}
          </Dropdown.RadioGroup>
        </Dropdown.Content>
      </Dropdown.Portal>
    </Dropdown.Root>
  );
}
