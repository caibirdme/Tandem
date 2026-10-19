import { Save, Sparkles } from 'lucide-react';
import { useState } from 'react';

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
      setNotice({ text: '标题总结模型已保存，下次生成标题时生效。', error: false });
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
          <h3>对话标题</h3>
          <p>让模型根据对话内容生成简短、易辨认的标题。</p>
        </div>
        <Sparkles size={20} />
      </div>
      <label>
        标题总结模型
        <select
          value={value}
          disabled={busy}
          onChange={(event) => {
            setValue(event.target.value);
            setNotice(null);
          }}
        >
          <option value="">跟随对话模型</option>
          {!available && (
            <option value={value} disabled>
              已保存的模型不可用，请重新选择
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
        <small>适用于聊天和工作。首轮回复完成后自动生成，也可在对话菜单中重新生成。</small>
      </label>
      <div className="info-box">
        <Sparkles size={16} />
        <p>标题总结会使用所选模型处理对话内容。手动命名的标题会保留，直到你主动重新生成。</p>
      </div>
      {notice && <Notice tone={notice.error ? 'error' : 'success'}>{notice.text}</Notice>}
      <div className="form-footer">
        <button className="button primary" type="submit" disabled={busy || !available}>
          {busy ? <Spinner /> : <Save size={15} />}保存标题设置
        </button>
      </div>
    </form>
  );
}
