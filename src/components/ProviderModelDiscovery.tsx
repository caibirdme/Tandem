import { ChevronRight, Download, RefreshCw } from 'lucide-react';
import { useEffect, useId, useReducer, useRef, useState } from 'react';

import { useI18n } from '../i18n';
import { dispatch, errorMessage } from '../lib/api';
import type { CredentialEditorState } from '../lib/credentialEditor';
import { emptyDiscovery, reduceDiscovery, selectedDiscoveryModels } from '../lib/modelDiscovery';
import { contextWindowKilotokens } from '../lib/settingsForm';
import type { DiscoveredModel, ProviderProtocol } from '../lib/types';
import { Modal, Notice, Spinner } from './ui';

import './ProviderModelDiscovery.css';

export function discoveryImportIssue(
  model: DiscoveredModel,
): 'settings.discoveryMissingContext' | 'settings.discoveryConflictingLimits' | null {
  if (model.contextWindow === undefined) return 'settings.discoveryMissingContext';
  if (model.maxOutputTokens !== undefined && model.maxOutputTokens >= model.contextWindow)
    return 'settings.discoveryConflictingLimits';
  return null;
}

export function DiscoveredModelRow({
  model,
  selected,
  existing,
  disabled,
  onToggle,
}: {
  model: DiscoveredModel;
  selected: boolean;
  existing: boolean;
  disabled: boolean;
  onToggle: () => void;
}) {
  const { t, locale } = useI18n();
  const [expanded, setExpanded] = useState(false);
  const detailsId = useId();
  const issue = discoveryImportIssue(model);
  const unavailable = existing || issue !== null;
  return (
    <li className="catalog-model">
      <div className="catalog-model-row">
        <label className="catalog-model-choice">
          <input
            type="checkbox"
            disabled={disabled || unavailable}
            checked={!unavailable && selected}
            onChange={onToggle}
          />
          <span className="catalog-model-name">
            <strong>{model.displayName}</strong>
            {existing && <small>{t('settings.modelAlreadyAdded')}</small>}
          </span>
          <span className="catalog-model-context">
            {model.contextWindow === undefined
              ? t('settings.notProvided')
              : `${contextWindowKilotokens(model.contextWindow)} K`}
          </span>
        </label>
        <button
          type="button"
          className="icon-button catalog-model-expand"
          aria-label={t(expanded ? 'settings.collapseModel' : 'settings.expandModel', {
            name: model.displayName,
          })}
          aria-expanded={expanded}
          aria-controls={detailsId}
          onClick={() => setExpanded(!expanded)}
        >
          <ChevronRight size={16} aria-hidden="true" />
        </button>
      </div>
      {issue && !existing && <p className="catalog-model-issue">{t(issue)}</p>}
      <div id={detailsId} className="catalog-model-details" hidden={!expanded}>
        <dl>
          <dt>{t('settings.modelId')}</dt>
          <dd>
            <code>{model.id}</code>
          </dd>
          <dt>{t('settings.contextWindow')}</dt>
          <dd>{model.contextWindow?.toLocaleString(locale) ?? t('settings.notProvided')}</dd>
          <dt>{t('settings.maxOutputTokens')}</dt>
          <dd>{model.maxOutputTokens?.toLocaleString(locale) ?? t('settings.notProvided')}</dd>
          <dt>{t('settings.supportedReasoning')}</dt>
          <dd>
            {model.supportedReasoningEfforts === undefined
              ? t('settings.notProvided')
              : model.supportedReasoningEfforts.length === 0
                ? t('settings.reasoningUnsupported')
                : model.supportedReasoningEfforts.join(', ')}
          </dd>
        </dl>
      </div>
    </li>
  );
}

export function ProviderModelDiscovery({
  baseUrl,
  listModelsPath,
  onListModelsPathChange,
  protocol,
  credential,
  hasSavedKey,
  disabled,
  existingIds,
  onAdd,
}: {
  baseUrl: string;
  listModelsPath: string;
  onListModelsPathChange: (path: string) => void;
  protocol: ProviderProtocol;
  credential: CredentialEditorState;
  hasSavedKey: boolean;
  disabled: boolean;
  existingIds: string[];
  onAdd: (models: DiscoveredModel[]) => void;
}) {
  const { t } = useI18n();
  const pathId = useId();
  const triggerRef = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
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

  function close() {
    request.current += 1;
    inFlight.current = false;
    update({ type: 'reset', request: request.current });
    setOpen(false);
  }

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

  const selected = selectedDiscoveryModels(state, existingIds).filter(
    (model) => discoveryImportIssue(model) === null,
  );
  const credentialHint = credentialPending
    ? t(
        ambiguousCredential
          ? 'settings.discoveryCredentialBlank'
          : 'settings.discoveryCredentialPending',
      )
    : null;
  return (
    <div className="model-discovery-trigger">
      <button
        ref={triggerRef}
        type="button"
        className="button"
        disabled={disabled || credentialPending}
        aria-describedby={credentialHint ? `${pathId}-credential` : undefined}
        onClick={() => {
          setOpen(true);
          void fetchModels();
        }}
      >
        <Download size={14} aria-hidden="true" />
        {t('settings.fetchModels')}
      </button>
      {credentialHint && <small id={`${pathId}-credential`}>{credentialHint}</small>}
      <Modal
        open={open}
        onOpenChange={(next) => {
          if (!next) close();
        }}
        title={t('settings.fetchModels')}
        description={t('settings.discoveryDescription')}
        returnFocusRef={triggerRef}
      >
        <section className="catalog-discovery" aria-busy={state.loading}>
          <div className="catalog-discovery-path">
            <label htmlFor={pathId}>{t('settings.listModelsPath')}</label>
            <div className="catalog-discovery-controls">
              <input
                id={pathId}
                value={listModelsPath}
                placeholder="/v1/models"
                aria-describedby={`${pathId}-hint`}
                onChange={(event) => onListModelsPathChange(event.target.value)}
              />
              <button
                type="button"
                className="button"
                disabled={disabled || state.loading || credentialPending}
                onClick={() => {
                  void fetchModels();
                }}
              >
                {state.loading ? <Spinner /> : <RefreshCw size={14} aria-hidden="true" />}
                {t('settings.refreshModels')}
              </button>
            </div>
            <small id={`${pathId}-hint`}>{t('settings.listModelsPathHint')}</small>
          </div>
          {state.error && <Notice>{state.error}</Notice>}
          {state.loading && (
            <output className="catalog-discovery-status">{t('settings.fetchingModels')}</output>
          )}
          {state.models !== null && (
            <>
              <p className="catalog-discovery-status" aria-live="polite">
                {state.models.length === 0
                  ? t('settings.noDiscoveredModels')
                  : t('settings.discoveryHint')}
              </p>
              <ul className="catalog-models">
                {state.models.map((model) => (
                  <DiscoveredModelRow
                    key={model.id}
                    model={model}
                    selected={state.selected.includes(model.id)}
                    existing={existingIds.includes(model.id)}
                    disabled={disabled || state.loading}
                    onToggle={() => update({ type: 'toggle', id: model.id })}
                  />
                ))}
              </ul>
            </>
          )}
        </section>
        <div className="catalog-discovery-footer">
          <button type="button" className="button" onClick={close}>
            {t('common.cancel')}
          </button>
          <button
            type="button"
            className="button primary"
            disabled={disabled || state.loading || selected.length === 0}
            onClick={() => {
              onAdd(selected);
              close();
            }}
          >
            {t('settings.addSelectedModels')}
            {selected.length > 0 && ` (${selected.length})`}
          </button>
        </div>
      </Modal>
    </div>
  );
}
