import { Eye, EyeOff } from 'lucide-react';
import { useId } from 'react';

import { useI18n } from '../i18n';
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
  const { t } = useI18n();
  const id = useId();
  return (
    <div className="credential-field">
      <label htmlFor={id}>{t('key.label')}</label>
      <div className="credential-input">
        <input
          id={id}
          type={state.visible ? 'text' : 'password'}
          autoComplete="new-password"
          autoCapitalize="none"
          spellCheck={false}
          value={state.removing ? '' : state.value}
          onChange={(event) => update({ type: 'edited', value: event.target.value })}
          placeholder={hasSavedKey ? t('key.keepPlaceholder') : t('key.emptyPlaceholder')}
          aria-describedby={`${id}-hint`}
        />
        <button
          className="icon-button credential-visibility"
          type="button"
          aria-label={state.visible ? t('key.hide') : t('key.show')}
          aria-pressed={state.visible}
          onClick={() => update({ type: 'visibilityToggled' })}
        >
          {state.visible ? <EyeOff size={16} /> : <Eye size={16} />}
        </button>
      </div>
      <small id={`${id}-hint`}>
        {state.removing
          ? t('key.removeHint')
          : hasSavedKey
            ? t('key.editHint')
            : t('key.localHint')}
      </small>
      {state.loadingRequest !== null && (
        <output className="credential-loading">
          <Spinner size={13} />
          {t('key.loading')}
        </output>
      )}
      {state.error && (
        <Notice>
          <div>{t('key.loadFailed', { error: state.error })}</div>
          <button className="text-button" type="button" onClick={onRetry}>
            {t('key.retry')}
          </button>
        </Notice>
      )}
      {hasSavedKey && (
        <button
          type="button"
          className="text-button key-remove"
          onClick={() => update({ type: 'removalToggled' })}
        >
          {state.removing ? t('key.cancelRemoval') : t('key.remove')}
        </button>
      )}
    </div>
  );
}
