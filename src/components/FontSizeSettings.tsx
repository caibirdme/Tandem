import { useState } from 'react';

import { useI18n } from '../i18n';
import { dispatch, errorMessage } from '../lib/api';
import type { FontSize } from '../lib/types';
import { Notice } from './ui';

export function FontSizeSettings({
  fontSize,
  onSaved,
}: {
  fontSize: FontSize;
  onSaved: () => Promise<unknown>;
}) {
  const { t } = useI18n();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function change(value: FontSize) {
    setBusy(true);
    setError(null);
    try {
      await dispatch('save_font_size', { fontSize: value });
      await onSaved();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="font-size-settings">
      <label>
        {t('settings.fontSize')}
        <select
          value={fontSize}
          disabled={busy}
          onChange={(event) => {
            void change(event.target.value as FontSize);
          }}
        >
          {(['small', 'medium', 'large'] as const).map((size) => (
            <option key={size} value={size}>
              {t(`settings.fontSize.${size}`)}
            </option>
          ))}
        </select>
        <small>{t('settings.fontSizeHint')}</small>
      </label>
      {error && <Notice>{error}</Notice>}
    </div>
  );
}
