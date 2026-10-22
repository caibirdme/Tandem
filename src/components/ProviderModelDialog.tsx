import { useId, useRef, useState, type RefObject } from 'react';

import { useI18n } from '../i18n';
import {
  changeModelFormId,
  initialModelForm,
  submitModelForm,
  type ModelFormError,
} from '../lib/modelForm';
import { reasoningEffortOptions } from '../lib/reasoningEffort';
import type { ModelFormDraft } from '../lib/settingsForm';
import type { ReasoningEffort } from '../lib/types';
import { Modal, Notice, RequiredMark } from './ui';

import './ProviderModelDialog.css';

interface ModelFormProps {
  model: ModelFormDraft | null;
  existingIds: string[];
  onSave: (model: ModelFormDraft) => void;
  onClose: () => void;
}

export function ProviderModelDialog({
  model,
  existingIds,
  onSave,
  onClose,
  returnFocusRef,
}: ModelFormProps & { returnFocusRef?: RefObject<HTMLElement | null> }) {
  const { t } = useI18n();
  const initialFocusRef = useRef<HTMLInputElement>(null);
  return (
    <Modal
      open
      onOpenChange={(open) => {
        if (!open) onClose();
      }}
      title={t(model ? 'settings.editModel' : 'settings.addModel')}
      description={t('settings.modelFormDescription')}
      initialFocusRef={initialFocusRef}
      {...(returnFocusRef ? { returnFocusRef } : {})}
    >
      <ProviderModelForm
        model={model}
        existingIds={existingIds}
        onSave={onSave}
        onClose={onClose}
        initialFocusRef={initialFocusRef}
      />
    </Modal>
  );
}

export function ProviderModelForm({
  model,
  existingIds,
  onSave,
  onClose,
  initialFocusRef,
}: ModelFormProps & { initialFocusRef?: RefObject<HTMLInputElement | null> }) {
  const { t } = useI18n();
  const id = useId();
  const [draft, setDraft] = useState(() => initialModelForm(model));
  const [error, setError] = useState<ModelFormError | null>(null);
  return (
    <form
      className="provider-model-form"
      onSubmit={(event) => {
        event.preventDefault();
        event.stopPropagation();
        const result = submitModelForm(draft, existingIds);
        if ('error' in result) {
          setError(result.error);
          return;
        }
        onSave(result.model);
      }}
    >
      <div className="provider-model-fields">
        <label className="provider-model-field">
          <span>
            {t('settings.modelId')}
            <RequiredMark />
          </span>
          <input
            ref={initialFocusRef}
            required
            maxLength={256}
            value={draft.id}
            placeholder={t('settings.modelExample')}
            onChange={(event) => setDraft(changeModelFormId(draft, event.target.value))}
          />
        </label>
        <label className="provider-model-field">
          <span>{t('settings.displayName')}</span>
          <input
            maxLength={256}
            value={draft.name}
            placeholder={t('settings.displayNameHint')}
            onChange={(event) => setDraft({ ...draft, name: event.target.value })}
          />
        </label>
        <label className="provider-model-field">
          <span>
            {t('settings.contextWindow')}
            <RequiredMark />
          </span>
          <span className="provider-model-control">
            <span className="context-window-input">
              <input
                required
                className="number-input"
                type="number"
                min="0.001"
                max="10000"
                step="0.001"
                value={draft.contextWindowK}
                aria-describedby={`${id}-context-hint`}
                onChange={(event) => setDraft({ ...draft, contextWindowK: event.target.value })}
              />
              <span aria-hidden="true">K</span>
            </span>
            <small id={`${id}-context-hint`}>{t('settings.contextWindowHint')}</small>
          </span>
        </label>
        <label className="provider-model-field">
          <span>{t('settings.maxOutputTokens')}</span>
          <input
            className="provider-model-number"
            type="number"
            min="1"
            max="10000000"
            step="1"
            value={draft.maxOutputTokens}
            placeholder={t('settings.default')}
            onChange={(event) => setDraft({ ...draft, maxOutputTokens: event.target.value })}
          />
        </label>
        <label className="provider-model-field">
          <span>{t('settings.reasoning')}</span>
          <span className="provider-model-control">
            <select
              value={draft.reasoningEffort ?? ''}
              aria-describedby={`${id}-reasoning-hint`}
              onChange={(event) =>
                setDraft({
                  ...draft,
                  reasoningEffort: (event.target.value || null) as ReasoningEffort,
                })
              }
            >
              <option value="">{t('settings.modelDefault')}</option>
              {reasoningEffortOptions(draft.supportedReasoningEfforts).map((effort) => (
                <option key={effort} value={effort}>
                  {effort}
                </option>
              ))}
            </select>
            <small id={`${id}-reasoning-hint`}>{t('settings.reasoningHint')}</small>
          </span>
        </label>
        {error && <Notice>{t(error)}</Notice>}
      </div>
      <div className="provider-model-footer">
        <button type="button" className="button" onClick={onClose}>
          {t('common.cancel')}
        </button>
        <button type="submit" className="button primary">
          {t('common.save')}
        </button>
      </div>
    </form>
  );
}
