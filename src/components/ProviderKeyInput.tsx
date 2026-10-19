import { Eye, EyeOff } from 'lucide-react';
import { useId } from 'react';

import type { CredentialEditorAction, CredentialEditorState } from '../lib/credentialEditor';
import { Notice, Spinner } from './ui';

export function ProviderKeyInput({
  state,
  hasSavedKey,
  update,
  onRetry,
}: {
  state: CredentialEditorState;
  hasSavedKey: boolean;
  update: (action: CredentialEditorAction) => void;
  onRetry: () => void;
}) {
  const id = useId();
  return (
    <div className="credential-field">
      <label htmlFor={id}>API Key</label>
      <div className="credential-input">
        <input
          id={id}
          type={state.visible ? 'text' : 'password'}
          autoComplete="new-password"
          autoCapitalize="none"
          spellCheck={false}
          value={state.removing ? '' : state.value}
          onChange={(event) => update({ type: 'edited', value: event.target.value })}
          placeholder={hasSavedKey ? '留空保留已保存的密钥' : '输入 API 密钥，本地服务可留空'}
          aria-describedby={`${id}-hint`}
        />
        <button
          className="icon-button credential-visibility"
          type="button"
          aria-label={state.visible ? '隐藏密钥' : '显示密钥'}
          aria-pressed={state.visible}
          onClick={() => update({ type: 'visibilityToggled' })}
        >
          {state.visible ? <EyeOff size={16} /> : <Eye size={16} />}
        </button>
      </div>
      <small id={`${id}-hint`}>
        {state.removing
          ? '保存后将清除现有密钥。'
          : hasSavedKey
            ? '密钥保存在本机。直接编辑可替换，留空或仅空格会保留现有密钥。'
            : '密钥保存在本机，编辑时默认隐藏。'}
      </small>
      {state.loadingRequest !== null && (
        <output className="credential-loading">
          <Spinner size={13} />
          正在读取已保存的密钥，也可以直接输入新密钥。
        </output>
      )}
      {state.error && (
        <Notice>
          <div>
            无法读取已保存的密钥：{state.error}。你仍可输入新密钥替换；留空保存不会更改现有密钥。
          </div>
          <button className="text-button" type="button" onClick={onRetry}>
            重新读取密钥
          </button>
        </Notice>
      )}
      {hasSavedKey && (
        <button
          type="button"
          className="text-button key-remove"
          onClick={() => update({ type: 'removalToggled' })}
        >
          {state.removing ? '取消清除密钥' : '清除已保存的密钥'}
        </button>
      )}
    </div>
  );
}
