import { Save, Sparkles } from 'lucide-react';
import { useState } from 'react';

import { useI18n } from '../i18n';
import { dispatch, errorMessage } from '../lib/api';
import { titleModelPayload, titleModelValue } from '../lib/settingsForm';
import type { Settings } from '../lib/types';
import { Notice, Spinner } from './ui';

export function TitlePreferences({
  settings,
  onSaved,
}: {
  settings: Settings;
  onSaved: () => Promise<unknown>;
}) {
  const { t } = useI18n();
  const [value, setValue] = useState(() => titleModelValue(settings.titleModel ?? null));
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null);
  const available =
    !value ||
    settings.providers.some((provider) =>
      provider.models.some(
        (model) => titleModelValue({ providerId: provider.id, modelId: model.id }) === value,
      ),
    );

  async function save() {
    setBusy(true);
    setNotice(null);
    try {
      await dispatch('save_title_model', titleModelPayload(value, settings.providers));
      await onSaved();
      setNotice({ text: t('titles.saved'), error: false });
    } catch (reason) {
      setNotice({ text: errorMessage(reason), error: true });
    } finally {
      setBusy(false);
    }
  }

  return (
    <form
      className="preferences-form"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <div className="settings-section-heading">
        <div>
          <h3>{t('common.conversationTitles')}</h3>
          <p>{t('titles.description')}</p>
        </div>
        <Sparkles size={20} />
      </div>
      <label>
        {t('titles.model')}
        <select
          value={value}
          disabled={busy}
          onChange={(event) => {
            setValue(event.target.value);
            setNotice(null);
          }}
        >
          <option value="">{t('titles.followModel')}</option>
          {!available && (
            <option value={value} disabled>
              {t('titles.unavailable')}
            </option>
          )}
          {settings.providers.map((provider) => (
            <optgroup key={provider.id} label={provider.name}>
              {provider.models.map((model) => (
                <option
                  key={model.id}
                  value={titleModelValue({ providerId: provider.id, modelId: model.id })}
                >
                  {model.name || model.id}
                </option>
              ))}
            </optgroup>
          ))}
        </select>
        <small>{t('titles.hint')}</small>
      </label>
      <div className="info-box">
        <Sparkles size={16} />
        <p>{t('titles.privacy')}</p>
      </div>
      {notice && <Notice tone={notice.error ? 'error' : 'success'}>{notice.text}</Notice>}
      <div className="form-footer">
        <button className="button primary" type="submit" disabled={busy || !available}>
          {busy ? <Spinner /> : <Save size={15} />}
          {t('titles.save')}
        </button>
      </div>
    </form>
  );
}
