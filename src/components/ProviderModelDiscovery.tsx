import { useEffect, useReducer, useRef } from 'react';

import { useI18n } from '../i18n';
import { dispatch, errorMessage } from '../lib/api';
import type { CredentialEditorState } from '../lib/credentialEditor';
import { emptyDiscovery, reduceDiscovery, selectedDiscoveryModels } from '../lib/modelDiscovery';
import type { DiscoveredModel, ProviderProtocol } from '../lib/types';

export function ProviderModelDiscovery({
  baseUrl,
  listModelsPath,
  protocol,
  credential,
  hasSavedKey,
  disabled,
  existingIds,
  onAdd,
}: {
  baseUrl: string;
  listModelsPath: string;
  protocol: ProviderProtocol;
  credential: CredentialEditorState;
  hasSavedKey: boolean;
  disabled: boolean;
  existingIds: string[];
  onAdd: (models: DiscoveredModel[]) => void;
}) {
  const { t } = useI18n();
  const [state, update] = useReducer(reduceDiscovery, emptyDiscovery);
  const request = useRef(0);
  const inFlight = useRef(false);
  const apiKey = credential.removing ? '' : credential.value.trim();
  const ambiguousCredential = hasSavedKey && credential.modified && !apiKey && !credential.removing;
  const credentialPending =
    credential.loadingRequest !== null || credential.error !== null || ambiguousCredential;
  useEffect(() => {
    request.current += 1;
    inFlight.current = false;
    update({ type: 'reset', request: request.current });
    return () => {
      request.current += 1;
      inFlight.current = false;
    };
  }, [baseUrl, listModelsPath, protocol, apiKey, credentialPending]);

  async function fetchModels() {
    if (inFlight.current || disabled || credentialPending) return;
    const id = ++request.current;
    try {
      const url = new URL(baseUrl.trim());
      if (
        !['http:', 'https:'].includes(url.protocol) ||
        !url.hostname ||
        url.username ||
        url.password ||
        url.search ||
        url.hash
      )
        throw new Error();
    } catch {
      update({ type: 'start', request: id });
      update({ type: 'failed', request: id, error: t('settings.discoveryInvalidUrl') });
      return;
    }
    inFlight.current = true;
    update({ type: 'start', request: id });
    try {
      const models = await dispatch('list_provider_models', {
        baseUrl: baseUrl.trim(),
        listModelsPath: listModelsPath.trim() || '/v1/models',
        apiKey,
      });
      if (request.current === id) update({ type: 'loaded', request: id, models });
    } catch (reason) {
      if (request.current === id)
        update({ type: 'failed', request: id, error: errorMessage(reason) });
    } finally {
      if (request.current === id) inFlight.current = false;
    }
  }
  const selected = selectedDiscoveryModels(state, existingIds);
  return (
    <section
      className="model-discovery"
      aria-label={t('settings.fetchModels')}
      aria-busy={state.loading}
    >
      <button
        type="button"
        className="text-button"
        disabled={disabled || state.loading || credentialPending}
        onClick={() => {
          void fetchModels();
        }}
      >
        {state.loading ? t('settings.fetchingModels') : t('settings.fetchModels')}
      </button>
      {credentialPending && (
        <p>
          {t(
            ambiguousCredential
              ? 'settings.discoveryCredentialBlank'
              : 'settings.discoveryCredentialPending',
          )}
        </p>
      )}
      {state.error && <p role="alert">{state.error}</p>}
      {state.models !== null && (
        <>
          <p aria-live="polite">
            {state.models.length === 0
              ? t('settings.noDiscoveredModels')
              : t('settings.discoveryHint')}
          </p>
          <div className="discovered-models">
            {state.models.map((model) => (
              <label key={model.id} className="discovered-model">
                <input
                  type="checkbox"
                  disabled={disabled || existingIds.includes(model.id)}
                  checked={!existingIds.includes(model.id) && state.selected.includes(model.id)}
                  onChange={() => update({ type: 'toggle', id: model.id })}
                />
                <span>
                  <strong>{model.displayName}</strong>
                  <code>{model.id}</code>
                  {existingIds.includes(model.id) && (
                    <small>{t('settings.modelAlreadyAdded')}</small>
                  )}
                </span>
              </label>
            ))}
          </div>
          {state.models.length > 0 && (
            <button
              type="button"
              className="text-button"
              disabled={disabled || selected.length === 0}
              onClick={() => {
                onAdd(selected);
                update({ type: 'added' });
              }}
            >
              {t('settings.addSelectedModels')}
            </button>
          )}
        </>
      )}
    </section>
  );
}
