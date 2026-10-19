import * as Dropdown from '@radix-ui/react-dropdown-menu';
import { Check, ChevronDown, ShieldAlert, ShieldCheck } from 'lucide-react';

import type { PermissionMode } from '../lib/types';
import { Spinner } from './ui';

const modes = [
  { value: 'ask', label: '逐次询问', description: '需要审批的工具操作逐次确认。' },
  {
    value: 'allow-all',
    label: '完全授权',
    description: '自动批准文件读写、命令等所有工具请求。',
  },
] as const;

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
  const authorized = mode === 'allow-all';
  const label = uncertain ? '待确认' : authorized ? '完全授权' : '逐次询问';
  return (
    <Dropdown.Root>
      <Dropdown.Trigger
        type="button"
        className={`permission-mode-selector ${authorized ? 'is-authorized' : ''}`}
        disabled={disabled || saving || uncertain}
        aria-label={`工具权限：${label}`}
        aria-busy={saving}
        title={
          uncertain
            ? '请先重新确认权限'
            : saving
              ? '正在更新工具权限…'
              : disabled
                ? '任务结束后可更改工具权限'
                : '更改工具权限'
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
          <Dropdown.Label className="dropdown-label">此工作的工具权限</Dropdown.Label>
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
