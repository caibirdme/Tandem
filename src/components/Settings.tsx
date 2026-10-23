import {
  ChartNoAxesCombined,
  Check,
  ChevronLeft,
  ChevronRight,
  ExternalLink,
  KeyRound,
  Plus,
  Radio,
  Save,
  Settings2,
  SlidersHorizontal,
  Sparkles,
  Trash2,
} from 'lucide-react';
import { useRef, useState } from 'react';

import { version } from '../../package.json';
import { useProviderCredential } from '../hooks/useProviderCredential';
import { useTelemetryCredential } from '../hooks/useTelemetryCredential';
import { useI18n } from '../i18n';
import { dispatch, errorMessage } from '../lib/api';
import {
  completeProviderEdit,
  contextWindowKilotokens,
  discoveredModelDraft,
  preferencesPayload,
  providerPayload,
  providerRequestUrl,
} from '../lib/settingsForm';
import type { ModelFormDraft, ProviderEditSession, ProviderFormDraft } from '../lib/settingsForm';
import type { Provider, ProviderProtocol, Settings as SettingsData, Telemetry } from '../lib/types';
import { BrowserLink } from './BrowserLink';
import { FontSizeSettings } from './FontSizeSettings';
import { LanguageSettings } from './LanguageSettings';
import { ProviderKeyInput } from './ProviderKeyInput';
import { ProviderModelDialog } from './ProviderModelDialog';
import { ProviderModelDiscovery } from './ProviderModelDiscovery';
import { TitlePreferences } from './TitlePreferences';
import { Modal, Notice, RequiredMark, Spinner, Toggle } from './ui';
import { UsageDashboard } from './UsageDashboard';

import './ProviderForm.css';

const protocolNames: Record<ProviderProtocol, string> = {
  'openai-completions': 'OpenAI Chat Completions',
  'openai-responses': 'OpenAI Responses',
  anthropic: 'Anthropic Messages',
};

export function Settings({
  open,
  onClose,
  settings,
  onSaved,
}: {
  open: boolean;
  onClose: () => void;
  settings: SettingsData;
  onSaved: () => Promise<unknown>;
}) {
  const { t } = useI18n();
  const [tab, setTab] = useState<'providers' | 'titles' | 'telemetry' | 'general' | 'usage'>(
    'providers',
  );
  const [editing, setEditing] = useState<ProviderEditSession | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Provider | null>(null);
  function editProvider(provider: Provider | null) {
    setEditing({ instanceId: crypto.randomUUID(), provider });
  }
  async function remove(provider: Provider) {
    setDeleting(provider.id);
    setError(null);
    try {
      await dispatch('delete_provider', { id: provider.id });
      await onSaved();
      setDeleteTarget(null);
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setDeleting(null);
    }
  }
  return (
    <Modal
      open={open}
      onOpenChange={(value) => {
        if (!value) onClose();
      }}
      title={t('common.settings')}
      description={t('settings.description')}
      wide
    >
      <div className="settings-layout">
        <nav className="settings-nav" aria-label={t('settings.categories')}>
          <button
            className={tab === 'providers' ? 'active' : ''}
            onClick={() => {
              setTab('providers');
              setEditing(null);
            }}
          >
            <SlidersHorizontal size={16} />
            {t('settings.providers')}
          </button>
          <button className={tab === 'usage' ? 'active' : ''} onClick={() => setTab('usage')}>
            <ChartNoAxesCombined size={16} />
            {t('usage.title')}
          </button>
          <button className={tab === 'titles' ? 'active' : ''} onClick={() => setTab('titles')}>
            <Sparkles size={16} />
            {t('common.conversationTitles')}
          </button>
          <button
            className={tab === 'telemetry' ? 'active' : ''}
            onClick={() => setTab('telemetry')}
          >
            <Radio size={16} />
            {t('common.observability')}
          </button>
          <button className={tab === 'general' ? 'active' : ''} onClick={() => setTab('general')}>
            <Settings2 size={16} />
            {t('common.general')}
          </button>
          <div className="settings-version">
            Tandem <span>{version}</span>
          </div>
        </nav>
        <div className="settings-content" key={tab}>
          {tab === 'providers' &&
            (editing ? (
              <ProviderEditor
                key={editing.instanceId}
                provider={editing.provider}
                onBack={() => setEditing(null)}
                onSaved={async () => {
                  await onSaved();
                  setEditing((current) => completeProviderEdit(current, editing.instanceId));
                }}
              />
            ) : (
              <>
                <div className="settings-section-heading">
                  <div>
                    <h3>{t('settings.providers')}</h3>
                    <p>{t('settings.providersDescription')}</p>
                  </div>
                  <button className="button compact" onClick={() => editProvider(null)}>
                    <Plus size={15} />
                    {t('settings.addProvider')}
                  </button>
                </div>
                {error && <Notice>{error}</Notice>}
                {settings.providers.length === 0 ? (
                  <div className="empty-providers">
                    <span className="empty-icon">
                      <SlidersHorizontal size={25} strokeWidth={1.4} />
                    </span>
                    <h4>{t('settings.firstProvider')}</h4>
                    <p>{t('settings.providerSupport')}</p>
                    <button className="button primary" onClick={() => editProvider(null)}>
                      <Plus size={15} />
                      {t('settings.addProvider')}
                    </button>
                    <div className="protocol-badges">
                      <span>Chat Completions</span>
                      <span>Responses</span>
                      <span>Anthropic</span>
                    </div>
                  </div>
                ) : (
                  <div className="provider-list">
                    {settings.providers.map((provider) => (
                      <div key={provider.id} className="provider-card">
                        <div className="provider-avatar">
                          {provider.name.slice(0, 1).toUpperCase()}
                        </div>
                        <button className="provider-details" onClick={() => editProvider(provider)}>
                          <strong>{provider.name}</strong>
                          <span>
                            {t('settings.providerModels', {
                              protocol: protocolNames[provider.protocol],
                              count: provider.models.length,
                            })}
                          </span>
                          <small>{provider.baseUrl}</small>
                        </button>
                        <span
                          className="key-status"
                          title={provider.hasApiKey ? t('settings.savedKey') : t('settings.noKey')}
                        >
                          <KeyRound size={13} />
                          {provider.hasApiKey ? t('settings.configured') : t('settings.keyless')}
                        </span>
                        <button
                          className="icon-button"
                          onClick={() => setDeleteTarget(provider)}
                          aria-label={t('settings.deleteProvider', { name: provider.name })}
                        >
                          <Trash2 size={15} />
                        </button>
                      </div>
                    ))}
                  </div>
                )}
                {deleteTarget && (
                  <div className="inline-confirm">
                    <p>{t('settings.deleteProviderWarning', { name: deleteTarget.name })}</p>
                    <div>
                      <button className="button compact" onClick={() => setDeleteTarget(null)}>
                        {t('common.cancel')}
                      </button>
                      <button
                        className="button destructive compact"
                        disabled={deleting !== null}
                        onClick={() => {
                          void remove(deleteTarget);
                        }}
                      >
                        {deleting && <Spinner />}
                        {t('settings.confirmDelete')}
                      </button>
                    </div>
                  </div>
                )}
              </>
            ))}
          {tab === 'usage' && <UsageDashboard />}
          {tab === 'telemetry' && (
            <Preferences key="telemetry" settings={settings} onSaved={onSaved} mode="telemetry" />
          )}
          {tab === 'titles' && <TitlePreferences settings={settings} onSaved={onSaved} />}
          {tab === 'general' && (
            <Preferences key="general" settings={settings} onSaved={onSaved} mode="general" />
          )}
        </div>
      </div>
    </Modal>
  );
}

export function ProviderEditor({
  provider,
  onBack,
  onSaved,
}: {
  provider: Provider | null;
  onBack: () => void;
  onSaved: () => Promise<void>;
}) {
  const { t } = useI18n();
  const [draft, setDraft] = useState<
    Omit<ProviderFormDraft, 'models'> & { models: (ModelFormDraft & { key: string })[] }
  >(() => ({
    id: provider?.id ?? '',
    name: provider?.name ?? '',
    protocol: provider?.protocol ?? 'openai-completions',
    baseUrl: provider?.baseUrl ?? '',
    listModelsPath: provider?.listModelsPath ?? '/v1/models',
    models: (provider?.models ?? []).map(({ contextWindow, ...model }) => ({
      ...model,
      contextWindowK: contextWindowKilotokens(contextWindow),
      key: crypto.randomUUID(),
    })),
  }));
  const credential = useProviderCredential(provider?.id ?? '', provider?.hasApiKey ?? false);
  const requestUrl = providerRequestUrl(draft.protocol, draft.baseUrl);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [modelEditor, setModelEditor] = useState<{
    key: string | null;
    model: ModelFormDraft | null;
  } | null>(null);
  const modelTrigger = useRef<HTMLElement | null>(null);

  async function save() {
    setError(null);
    try {
      const payload = providerPayload(draft, credential.edit);
      setBusy(true);
      await dispatch('save_provider', payload);
      await onSaved();
    } catch (reason) {
      setError(errorMessage(reason));
    } finally {
      setBusy(false);
    }
  }
  return (
    <>
      <form
        className="provider-form"
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <button className="text-button back-button" type="button" onClick={onBack}>
          <ChevronLeft size={15} />
          {t('settings.allProviders')}
        </button>
        <h3>{provider ? t('settings.editProvider') : t('settings.addProvider')}</h3>
        <div className="provider-fields">
          <label className="provider-field">
            <span>
              {t('settings.name')}
              <RequiredMark />
            </span>
            <input
              required
              value={draft.name}
              onChange={(event) => setDraft({ ...draft, name: event.target.value })}
              placeholder={t('settings.providerExample')}
            />
          </label>
          <label className="provider-field">
            <span>{t('settings.protocol')}</span>
            <select
              value={draft.protocol}
              onChange={(event) =>
                setDraft({ ...draft, protocol: event.target.value as ProviderProtocol })
              }
            >
              {Object.entries(protocolNames).map(([value, label]) => (
                <option key={value} value={value}>
                  {label}
                </option>
              ))}
            </select>
          </label>
          <label className="provider-field">
            <span>
              {t('settings.baseUrl')}
              <RequiredMark />
            </span>
            <input
              required
              type="url"
              value={draft.baseUrl}
              onChange={(event) => setDraft({ ...draft, baseUrl: event.target.value })}
              placeholder={
                draft.protocol === 'anthropic'
                  ? 'https://api.anthropic.com'
                  : 'https://api.openai.com/v1'
              }
            />
            {requestUrl ? (
              <small className="provider-request-url" aria-live="polite">
                <span>{t('settings.requestUrl')}</span>
                <code>{requestUrl}</code>
              </small>
            ) : (
              <small>{t('settings.baseUrlHint')}</small>
            )}
          </label>
          <ProviderKeyInput
            state={credential.state}
            hasSavedKey={provider?.hasApiKey ?? false}
            update={credential.update}
            onRetry={credential.retry}
          />
        </div>
        <div className="provider-model-section">
          <div className="models-heading">
            <div>
              <h4>{t('settings.models')}</h4>
              <p>{t('settings.modelListHint')}</p>
            </div>
            <span className="provider-model-count">{draft.models.length}</span>
          </div>
          <div className="provider-model-actions">
            <ProviderModelDiscovery
              baseUrl={draft.baseUrl}
              listModelsPath={draft.listModelsPath}
              onListModelsPathChange={(listModelsPath) =>
                setDraft((current) => ({ ...current, listModelsPath }))
              }
              protocol={draft.protocol}
              credential={credential.state}
              hasSavedKey={provider?.hasApiKey ?? false}
              disabled={busy}
              existingIds={draft.models.map((model) => model.id.trim())}
              onAdd={(models) => {
                setError(null);
                setDraft((current) => ({
                  ...current,
                  models: [
                    ...current.models,
                    ...models
                      .filter(
                        (model) =>
                          !current.models.some((existing) => existing.id.trim() === model.id),
                      )
                      .map((model) => ({
                        ...discoveredModelDraft(model),
                        key: crypto.randomUUID(),
                      })),
                  ],
                }));
              }}
            />
            <button
              className="button"
              type="button"
              disabled={busy}
              onClick={(event) => {
                modelTrigger.current = event.currentTarget;
                setModelEditor({ key: null, model: null });
              }}
            >
              <Plus size={14} aria-hidden="true" />
              {t('settings.addModel')}
            </button>
          </div>
          {draft.models.length === 0 ? (
            <p className="provider-model-empty">{t('settings.noAddedModels')}</p>
          ) : (
            <ul className="provider-model-list" aria-label={t('settings.models')}>
              {draft.models.map((model, index) => (
                <li key={model.key}>
                  <button
                    className="provider-model-open"
                    type="button"
                    disabled={busy}
                    onClick={(event) => {
                      modelTrigger.current = event.currentTarget;
                      setModelEditor({ key: model.key, model });
                    }}
                  >
                    <span className="provider-model-name">
                      <strong>{model.name || model.id}</strong>
                      <code>{model.id}</code>
                    </span>
                    <span className="provider-model-context">{model.contextWindowK} K</span>
                    <ChevronRight size={16} aria-hidden="true" />
                  </button>
                  <button
                    className="icon-button"
                    type="button"
                    disabled={busy}
                    aria-label={t('settings.removeModel', { number: index + 1 })}
                    onClick={() =>
                      setDraft((current) => ({
                        ...current,
                        models: current.models.filter((value) => value.key !== model.key),
                      }))
                    }
                  >
                    <Trash2 size={14} />
                  </button>
                </li>
              ))}
            </ul>
          )}
        </div>
        {error && <Notice>{error}</Notice>}
        <div className="form-footer">
          <button className="button" type="button" onClick={onBack}>
            {t('common.cancel')}
          </button>
          <button className="button primary" type="submit" disabled={busy}>
            {busy ? <Spinner /> : <Check size={15} />}
            {t('settings.saveProvider')}
          </button>
        </div>
      </form>
      {modelEditor && (
        <ProviderModelDialog
          model={modelEditor.model}
          existingIds={draft.models
            .filter((model) => model.key !== modelEditor.key)
            .map((model) => model.id.trim())}
          returnFocusRef={modelTrigger}
          onClose={() => setModelEditor(null)}
          onSave={(model) => {
            setError(null);
            setDraft((current) => ({
              ...current,
              models: modelEditor.key
                ? current.models.map((value) =>
                    value.key === modelEditor.key ? { ...model, key: value.key } : value,
                  )
                : [...current.models, { ...model, key: crypto.randomUUID() }],
            }));
            setModelEditor(null);
          }}
        />
      )}
    </>
  );
}

export function Preferences({
  settings,
  onSaved,
  mode,
}: {
  settings: SettingsData;
  onSaved: () => Promise<unknown>;
  mode: 'telemetry' | 'general';
}) {
  const { t } = useI18n();
  const [telemetry, setTelemetry] = useState(settings.telemetry);
  const isGrpc = !telemetry.agentlens && telemetry.protocol === 'grpc';
  const credential = useTelemetryCredential(mode === 'telemetry' && !!telemetry.agentlens);
  const [cliPath, setCliPath] = useState(settings.cliPath ?? '');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null);
  function update(patch: Partial<Telemetry>) {
    setTelemetry((current) => ({ ...current, ...patch }));
    setNotice(null);
  }
  async function save() {
    if (busy) return;
    const submittedCredential = credential.state;
    setBusy(true);
    setNotice(null);
    try {
      if (
        mode === 'telemetry' &&
        telemetry.enabled &&
        telemetry.agentlens &&
        (credential.edit.kind === 'remove' ||
          (!credential.hasSavedKey && credential.edit.kind === 'keep'))
      ) {
        throw new Error(t('error.telemetryApiKey'));
      }
      await dispatch(
        'save_preferences',
        preferencesPayload(
          settings,
          mode === 'telemetry'
            ? { kind: 'telemetry', value: telemetry, credential: credential.edit }
            : { kind: 'cliPath', value: cliPath },
        ),
      );
      await onSaved();
      if (mode === 'telemetry' && telemetry.agentlens) credential.saved(submittedCredential);
      setNotice({ text: t('settings.saved'), error: false });
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
      {mode === 'telemetry' ? (
        <>
          <div className="settings-section-heading">
            <div>
              <h3>{t('common.observability')}</h3>
              <p>{t('settings.telemetryDescription')}</p>
            </div>
            <Radio size={20} />
          </div>
          <Toggle
            checked={telemetry.enabled}
            onCheckedChange={(enabled) => update({ enabled })}
            label={t('settings.enableTelemetry')}
            description={
              telemetry.agentlens
                ? t('settings.agentlensTelemetryHint')
                : t('settings.telemetryHint')
            }
          />
          <label>
            {t('settings.telemetryDestination')}
            <select
              value={telemetry.agentlens ? 'agentlens' : 'otlp'}
              onChange={(event) =>
                update({
                  agentlens: event.target.value === 'agentlens' ? { serviceName: 'tandem' } : null,
                  ...(event.target.value === 'agentlens' ? { protocol: 'http/json' } : {}),
                })
              }
            >
              <option value="otlp">{t('settings.otlpCollector')}</option>
              <option value="agentlens">AgentLens</option>
            </select>
          </label>
          {telemetry.agentlens && (
            <>
              <label>
                <span>
                  {t('settings.telemetryServiceName')}
                  {telemetry.enabled && <RequiredMark />}
                </span>
                <input
                  required={telemetry.enabled}
                  value={telemetry.agentlens.serviceName}
                  onChange={(event) => update({ agentlens: { serviceName: event.target.value } })}
                  placeholder="tandem"
                />
                <small>{t('settings.agentlensHint')}</small>
              </label>
              <ProviderKeyInput
                label={t('settings.agentlensKey')}
                emptyPlaceholder={t('settings.agentlensKeyPlaceholder')}
                state={credential.state}
                hasSavedKey={credential.hasSavedKey}
                update={credential.update}
                onRetry={credential.retry}
              />
            </>
          )}
          <label>
            <span>
              {t('settings.endpoint')}
              {telemetry.enabled && <RequiredMark />}
            </span>
            <input
              type="url"
              required={telemetry.enabled}
              value={telemetry.endpoint}
              onChange={(event) => update({ endpoint: event.target.value })}
              placeholder={isGrpc ? 'http://localhost:4317' : 'http://localhost:4318'}
            />
            <small>{t(isGrpc ? 'settings.endpointGrpcHint' : 'settings.endpointHint')}</small>
          </label>
          {!telemetry.agentlens && (
            <label>
              {t('settings.transportProtocol')}
              <select
                value={telemetry.protocol}
                onChange={(event) =>
                  update({ protocol: event.target.value as Telemetry['protocol'] })
                }
              >
                <option value="http/protobuf">HTTP / Protobuf</option>
                <option value="http/json">HTTP / JSON</option>
                <option value="grpc">gRPC</option>
              </select>
            </label>
          )}
          <Toggle
            checked={telemetry.captureContent}
            onCheckedChange={(captureContent) => update({ captureContent })}
            label={t('settings.captureContent')}
            description={t('settings.captureContentHint')}
          />
          <div className="info-box">
            <Radio size={16} />
            <div>
              <p>{t('settings.telemetryLocation')}</p>
              {telemetry.agentlens && <p>{t('settings.agentlensScope')}</p>}
            </div>
          </div>
        </>
      ) : (
        <>
          <div className="settings-section-heading">
            <div>
              <h3>{t('common.general')}</h3>
              <p>{t('settings.generalDescription')}</p>
            </div>
          </div>
          <LanguageSettings />
          <FontSizeSettings fontSize={settings.fontSize} onSaved={onSaved} />
          <label>
            {t('settings.cliPath')}
            <input
              value={cliPath}
              onChange={(event) => {
                setCliPath(event.target.value);
                setNotice(null);
              }}
              placeholder={t('settings.cliDefault')}
            />
            <small>{t('settings.cliHint')}</small>
          </label>
          <div className="about-card">
            <img className="brand-symbol" src="/tandem.svg" alt="" />
            <div>
              <h4>Tandem</h4>
              <p>{t('settings.tagline')}</p>
              <span>{t('settings.sdkVersion')}</span>
            </div>
          </div>
          <BrowserLink className="text-button" href="https://github.com/github/copilot-sdk">
            {t('settings.sdkDocs')}
            <ExternalLink size={13} />
          </BrowserLink>
        </>
      )}
      {notice && <Notice tone={notice.error ? 'error' : 'success'}>{notice.text}</Notice>}
      <div className="form-footer">
        <button className="button primary" type="submit" disabled={busy}>
          {busy ? <Spinner /> : <Save size={15} />}
          {t('settings.save')}
        </button>
      </div>
    </form>
  );
}
