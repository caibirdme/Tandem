import {
  Check,
  ChevronLeft,
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
import { useState } from 'react';

import { version } from '../../package.json';
import { useProviderCredential } from '../hooks/useProviderCredential';
import { dispatch, errorMessage } from '../lib/api';
import { completeProviderEdit, preferencesPayload, providerPayload } from '../lib/settingsForm';
import type { ProviderEditSession } from '../lib/settingsForm';
import type {
  Model,
  Provider,
  ProviderProtocol,
  Settings as SettingsData,
  Telemetry,
} from '../lib/types';
import { BrowserLink } from './BrowserLink';
import { ProviderKeyInput } from './ProviderKeyInput';
import { TitlePreferences } from './TitlePreferences';
import { Modal, Notice, Spinner, Toggle } from './ui';

const protocolNames: Record<ProviderProtocol, string> = {
  'openai-completions': 'OpenAI Chat Completions',
  'openai-responses': 'OpenAI Responses',
  anthropic: 'Anthropic Messages',
};
const blankModel = (): Model => ({
  id: '',
  name: '',
  reasoningEffort: null,
  contextWindow: null,
  maxOutputTokens: null,
});
const blankProvider = (): Provider => ({
  id: '',
  name: '',
  protocol: 'openai-completions',
  baseUrl: '',
  hasApiKey: false,
  models: [blankModel()],
});

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
  const [tab, setTab] = useState<'providers' | 'titles' | 'telemetry' | 'general'>('providers');
  const [editing, setEditing] = useState<ProviderEditSession | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [deleting, setDeleting] = useState<string | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<Provider | null>(null);
  function editProvider(provider: Provider) {
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
      title="设置"
      description="连接适合你的模型与工具。"
      wide
    >
      <div className="settings-layout">
        <nav className="settings-nav" aria-label="设置分类">
          <button
            className={tab === 'providers' ? 'active' : ''}
            onClick={() => {
              setTab('providers');
              setEditing(null);
            }}
          >
            <SlidersHorizontal size={16} />
            模型供应商
          </button>
          <button className={tab === 'titles' ? 'active' : ''} onClick={() => setTab('titles')}>
            <Sparkles size={16} />
            对话标题
          </button>
          <button
            className={tab === 'telemetry' ? 'active' : ''}
            onClick={() => setTab('telemetry')}
          >
            <Radio size={16} />
            可观测性
          </button>
          <button className={tab === 'general' ? 'active' : ''} onClick={() => setTab('general')}>
            <Settings2 size={16} />
            通用
          </button>
          <div className="settings-version">
            Relay <span>{version}</span>
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
                    <h3>模型供应商</h3>
                    <p>使用你自己的 API 密钥。密钥保存在本机，编辑时默认隐藏。</p>
                  </div>
                  <button className="button compact" onClick={() => editProvider(blankProvider())}>
                    <Plus size={15} />
                    添加供应商
                  </button>
                </div>
                {error && <Notice>{error}</Notice>}
                {settings.providers.length === 0 ? (
                  <div className="empty-providers">
                    <span className="empty-icon">
                      <SlidersHorizontal size={25} strokeWidth={1.4} />
                    </span>
                    <h4>从连接第一个模型开始</h4>
                    <p>
                      支持 OpenAI 兼容协议和 Anthropic，
                      <br />
                      也可以连接你自己的模型网关。
                    </p>
                    <button
                      className="button primary"
                      onClick={() => editProvider(blankProvider())}
                    >
                      <Plus size={15} />
                      添加供应商
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
                            {protocolNames[provider.protocol]} · {provider.models.length} 个模型
                          </span>
                          <small>{provider.baseUrl}</small>
                        </button>
                        <span
                          className="key-status"
                          title={provider.hasApiKey ? '已保存 API 密钥' : '未设置密钥'}
                        >
                          <KeyRound size={13} />
                          {provider.hasApiKey ? '已配置' : '无密钥'}
                        </span>
                        <button
                          className="icon-button"
                          onClick={() => setDeleteTarget(provider)}
                          aria-label={`删除 ${provider.name}`}
                        >
                          <Trash2 size={15} />
                        </button>
                      </div>
                    ))}
                  </div>
                )}
                {deleteTarget && (
                  <div className="inline-confirm">
                    <p>删除「{deleteTarget.name}」？使用它的历史对话将无法继续运行。</p>
                    <div>
                      <button className="button compact" onClick={() => setDeleteTarget(null)}>
                        取消
                      </button>
                      <button
                        className="button destructive compact"
                        disabled={deleting !== null}
                        onClick={() => {
                          void remove(deleteTarget);
                        }}
                      >
                        {deleting && <Spinner />}确认删除
                      </button>
                    </div>
                  </div>
                )}
              </>
            ))}
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

function ProviderEditor({
  provider,
  onBack,
  onSaved,
}: {
  provider: Provider;
  onBack: () => void;
  onSaved: () => Promise<void>;
}) {
  const [draft, setDraft] = useState({
    ...provider,
    models: provider.models.map((model) => ({ ...model, key: crypto.randomUUID() })),
  });
  const credential = useProviderCredential(provider.id, provider.hasApiKey);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  function modelChange(index: number, patch: Partial<Model>) {
    setDraft((current) => ({
      ...current,
      models: current.models.map((model, i) => (i === index ? { ...model, ...patch } : model)),
    }));
  }
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
    <form
      className="provider-form"
      onSubmit={(event) => {
        event.preventDefault();
        void save();
      }}
    >
      <button className="text-button back-button" type="button" onClick={onBack}>
        <ChevronLeft size={15} />
        所有供应商
      </button>
      <h3>{provider.id ? '编辑供应商' : '添加供应商'}</h3>
      <div className="form-grid">
        <label>
          名称
          <input
            required
            value={draft.name}
            onChange={(event) => setDraft({ ...draft, name: event.target.value })}
            placeholder="例如：我的 OpenAI"
          />
        </label>
        <label>
          API 协议
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
      </div>
      <label>
        Base URL
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
        <small>填写 API 根地址，例如 https://api.openai.com/v1。</small>
      </label>
      <ProviderKeyInput
        state={credential.state}
        hasSavedKey={provider.hasApiKey}
        update={credential.update}
        onRetry={credential.retry}
      />
      <div className="models-heading">
        <div>
          <h4>模型</h4>
          <p>模型 ID 须与你的供应商保持一致。</p>
        </div>
        <button
          className="text-button"
          type="button"
          onClick={() =>
            setDraft({
              ...draft,
              models: [...draft.models, { ...blankModel(), key: crypto.randomUUID() }],
            })
          }
        >
          <Plus size={14} />
          添加模型
        </button>
      </div>
      {draft.models.map((model, index) => (
        <fieldset className="model-editor" key={model.key}>
          <legend>模型 {index + 1}</legend>
          {draft.models.length > 1 && (
            <button
              className="icon-button model-delete"
              type="button"
              aria-label={`移除模型 ${index + 1}`}
              onClick={() =>
                setDraft({ ...draft, models: draft.models.filter((_, i) => i !== index) })
              }
            >
              <Trash2 size={14} />
            </button>
          )}
          <div className="form-grid">
            <label>
              模型 ID
              <input
                required
                value={model.id}
                onChange={(event) => modelChange(index, { id: event.target.value })}
                placeholder="例如：gpt-5.4"
              />
            </label>
            <label>
              显示名称
              <input
                value={model.name}
                onChange={(event) => modelChange(index, { name: event.target.value })}
                placeholder="可选，默认使用模型 ID"
              />
            </label>
          </div>
          <details className="advanced-model">
            <summary>高级参数</summary>
            <div className="form-grid three">
              <label>
                思考强度
                <select
                  value={model.reasoningEffort ?? ''}
                  onChange={(event) =>
                    modelChange(index, {
                      reasoningEffort: (event.target.value || null) as Model['reasoningEffort'],
                    })
                  }
                >
                  <option value="">模型默认</option>
                  <option value="low">低</option>
                  <option value="medium">中</option>
                  <option value="high">高</option>
                </select>
              </label>
              <label>
                上下文窗口
                <input
                  type="number"
                  min="1"
                  step="1"
                  value={model.contextWindow ?? ''}
                  onChange={(event) =>
                    modelChange(index, {
                      contextWindow: event.target.value ? Number(event.target.value) : null,
                    })
                  }
                  placeholder="默认"
                />
              </label>
              <label>
                最大输出 Token
                <input
                  type="number"
                  min="1"
                  step="1"
                  value={model.maxOutputTokens ?? ''}
                  onChange={(event) =>
                    modelChange(index, {
                      maxOutputTokens: event.target.value ? Number(event.target.value) : null,
                    })
                  }
                  placeholder="默认"
                />
              </label>
            </div>
            <p>仅在模型支持时设置这些参数。</p>
          </details>
        </fieldset>
      ))}
      {error && <Notice>{error}</Notice>}
      <div className="form-footer">
        <button className="button" type="button" onClick={onBack}>
          取消
        </button>
        <button className="button primary" type="submit" disabled={busy}>
          {busy ? <Spinner /> : <Check size={15} />}保存供应商
        </button>
      </div>
    </form>
  );
}

function Preferences({
  settings,
  onSaved,
  mode,
}: {
  settings: SettingsData;
  onSaved: () => Promise<unknown>;
  mode: 'telemetry' | 'general';
}) {
  const [telemetry, setTelemetry] = useState(settings.telemetry);
  const [cliPath, setCliPath] = useState(settings.cliPath ?? '');
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null);
  function update(patch: Partial<Telemetry>) {
    setTelemetry((current) => ({ ...current, ...patch }));
    setNotice(null);
  }
  async function save() {
    setBusy(true);
    setNotice(null);
    try {
      await dispatch(
        'save_preferences',
        preferencesPayload(
          settings,
          mode === 'telemetry'
            ? { kind: 'telemetry', value: telemetry }
            : { kind: 'cliPath', value: cliPath },
        ),
      );
      await onSaved();
      setNotice({ text: '设置已保存，将在下一次会话运行时生效。', error: false });
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
              <h3>可观测性</h3>
              <p>通过 OpenTelemetry 导出 Agent 的运行轨迹。</p>
            </div>
            <Radio size={20} />
          </div>
          <Toggle
            checked={telemetry.enabled}
            onCheckedChange={(enabled) => update({ enabled })}
            label="启用遥测"
            description="将追踪与指标发送到你的 OTLP Collector。"
          />
          <label>
            导出地址
            <input
              type="url"
              required={telemetry.enabled}
              value={telemetry.endpoint}
              onChange={(event) => update({ endpoint: event.target.value })}
              placeholder="http://localhost:4318"
            />
            <small>使用 Collector 的 HTTP 根地址，SDK 会添加对应信号路径。</small>
          </label>
          <label>
            传输协议
            <select
              value={telemetry.protocol}
              onChange={(event) =>
                update({ protocol: event.target.value as Telemetry['protocol'] })
              }
            >
              <option value="http/protobuf">HTTP / Protobuf</option>
              <option value="http/json">HTTP / JSON</option>
            </select>
          </label>
          <Toggle
            checked={telemetry.captureContent}
            onCheckedChange={(captureContent) => update({ captureContent })}
            label="包含对话内容"
            description="导出提示词、模型回复和工具参数。内容可能包含项目中的敏感信息。"
          />
          <div className="info-box">
            <Radio size={16} />
            <p>本地会话由本机导出；远程工作由远端服务器导出。请确保运行环境能够访问这个地址。</p>
          </div>
        </>
      ) : (
        <>
          <div className="settings-section-heading">
            <div>
              <h3>通用</h3>
              <p>调整你的 Agent 运行环境。</p>
            </div>
          </div>
          <label>
            Copilot CLI 路径（可选）
            <input
              value={cliPath}
              onChange={(event) => {
                setCliPath(event.target.value);
                setNotice(null);
              }}
              placeholder="默认使用应用内置的 Copilot CLI"
            />
            <small>
              应用已内置匹配版本，无需额外安装；仅在调试自定义 CLI 时填写可执行文件的完整路径。
            </small>
          </label>
          <div className="about-card">
            <span className="brand-symbol">
              r<span />
            </span>
            <div>
              <h4>Relay</h4>
              <p>让想法与行动相连。</p>
              <span>基于 GitHub Copilot SDK · v1.0.23-preview.5</span>
            </div>
          </div>
          <BrowserLink className="text-button" href="https://github.com/github/copilot-sdk">
            Copilot SDK 文档
            <ExternalLink size={13} />
          </BrowserLink>
        </>
      )}
      {notice && <Notice tone={notice.error ? 'error' : 'success'}>{notice.text}</Notice>}
      <div className="form-footer">
        <button className="button primary" type="submit" disabled={busy}>
          {busy ? <Spinner /> : <Save size={15} />}保存设置
        </button>
      </div>
    </form>
  );
}
