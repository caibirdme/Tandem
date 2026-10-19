import * as Dropdown from '@radix-ui/react-dropdown-menu';
import {
  ArrowDown,
  ArrowRight,
  ArrowUp,
  Check,
  CheckCheck,
  ChevronDown,
  Code2,
  Compass,
  Copy,
  FileText,
  Folder,
  Lightbulb,
  LockKeyhole,
  MessageSquare,
  Plus,
  Server,
  ShieldCheck,
  Sparkles,
  Square,
  Terminal,
  Wrench,
} from 'lucide-react';
import { lazy, Suspense, useEffect, useRef, useState } from 'react';

import { errorMessage } from '../lib/api';
import type { Conversation } from '../lib/conversation';
import { needsWorkPermissionConfirmation } from '../lib/sessionOptions';
import type {
  ModelSelection,
  PermissionMode,
  Project,
  Provider,
  RuntimeStatus,
  Session,
} from '../lib/types';
import { PermissionModeSelector } from './PermissionModeSelector';
import { Modal, Notice, Spinner } from './ui';

interface ChatProps {
  session: Session | undefined;
  project: Project | undefined;
  providers: Provider[];
  selection: ModelSelection | null;
  onSelection: (selection: ModelSelection) => void;
  conversation: Conversation;
  historyLoading: boolean;
  historyLoaded: boolean;
  onReloadHistory: () => void;
  sending: boolean;
  permissionModeSaving: boolean;
  permissionModeError: string | null;
  modelSaving: boolean;
  modelError: string | null;
  onConfirmModel: () => void;
  onPermissionMode: (mode: PermissionMode) => void;
  onConfirmPermissionMode: () => void;
  runtime: RuntimeStatus | undefined;
  installing: boolean;
  onInstall: () => void;
  onSettings: () => void;
  onSend: (content: string, permissionMode: PermissionMode) => Promise<void>;
  onAbort: () => void;
  onPermission: (requestId: string, approved: boolean) => Promise<void>;
  connected: boolean;
}

export function Chat(props: ChatProps) {
  const [draft, setDraft] = useState('');
  const [draftPermissionMode, setDraftPermissionMode] = useState<PermissionMode>('ask');
  const [jumpVisible, setJumpVisible] = useState(false);
  const [pendingPermission, setPendingPermission] = useState<string | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const [workPermissionPromptOpen, setWorkPermissionPromptOpen] = useState(false);
  const transcript = useRef<HTMLDivElement>(null);
  const textarea = useRef<HTMLTextAreaElement>(null);
  const cancelWorkPermission = useRef<HTMLButtonElement>(null);
  const pendingWorkContent = useRef<string | null>(null);
  const submitting = useRef(false);
  const stickToBottom = useRef(true);
  const running =
    props.sending || props.conversation.status === 'running' || props.session?.status === 'running';
  const permissionMode = props.project
    ? (props.session?.permissionMode ?? draftPermissionMode)
    : 'ask';
  const workHistoryUncertain = Boolean(
    props.project &&
    props.session &&
    permissionMode === 'ask' &&
    !props.historyLoaded &&
    !props.conversation.messages.some((message) => message.role === 'user'),
  );
  const permissionModeDisabled =
    running ||
    props.historyLoading ||
    props.permissionModeSaving ||
    Boolean(props.permissionModeError) ||
    props.modelSaving ||
    Boolean(props.modelError) ||
    !props.connected ||
    props.conversation.permissions.length > 0;
  const hasMessages = Boolean(
    props.conversation.messages.length ||
    props.conversation.stream ||
    props.conversation.failedSend,
  );
  const currentProvider = props.providers.find(
    (provider) => provider.id === props.selection?.providerId,
  );
  const currentModel = currentProvider?.models.find(
    (model) => model.id === props.selection?.modelId,
  );
  const modelDisabled = permissionModeDisabled;
  const modelLabel = props.modelError
    ? '待确认'
    : currentModel?.name || currentModel?.id || props.selection?.modelId || '选择模型';
  const modelHint = running
    ? '请先停止当前回复或等待结束，再切换模型'
    : props.historyLoading
      ? '正在载入对话，请稍后切换模型'
      : props.modelSaving
        ? '正在保存模型选择…'
        : props.modelError
          ? '请先重新确认当前模型'
          : '选择模型，下一条消息将使用新模型';
  // Sending initializes the local or remote runtime on demand.
  const canSend = Boolean(
    draft.trim() &&
    props.selection &&
    currentModel &&
    props.connected &&
    !running &&
    !props.historyLoading &&
    !workHistoryUncertain &&
    !props.permissionModeSaving &&
    !props.permissionModeError &&
    !props.modelSaving &&
    !props.modelError &&
    props.conversation.permissions.length === 0,
  );

  useEffect(() => {
    if (stickToBottom.current && transcript.current)
      transcript.current.scrollTop = transcript.current.scrollHeight;
  }, [
    props.conversation.messages,
    props.conversation.stream,
    props.conversation.tools,
    props.conversation.permissions,
  ]);
  useEffect(() => {
    textarea.current?.focus();
  }, []);
  useEffect(() => {
    if (textarea.current) {
      textarea.current.style.height = 'auto';
      textarea.current.style.height = `${Math.min(textarea.current.scrollHeight, 190)}px`;
    }
  }, [draft]);

  async function submit() {
    if (!canSend || submitting.current || pendingWorkContent.current !== null) return;
    const content = draft.trim();
    if (
      needsWorkPermissionConfirmation({
        projectId: props.project?.id ?? null,
        sessionId: props.session?.id ?? null,
        permissionMode,
        historyLoaded: props.historyLoaded,
        messages: props.conversation.messages,
      })
    ) {
      pendingWorkContent.current = content;
      setWorkPermissionPromptOpen(true);
      return;
    }
    await sendDraft(content, permissionMode);
  }
  async function sendDraft(content: string, mode: PermissionMode) {
    if (submitting.current) return;
    submitting.current = true;
    setDraft('');
    setLocalError(null);
    stickToBottom.current = true;
    try {
      await props.onSend(content, mode);
    } catch (reason) {
      setDraft((current) => current || content);
      setLocalError(errorMessage(reason));
    } finally {
      submitting.current = false;
    }
  }
  function cancelWorkPrompt() {
    pendingWorkContent.current = null;
    setWorkPermissionPromptOpen(false);
  }
  async function confirmWorkPermission(mode: PermissionMode) {
    const content = pendingWorkContent.current;
    if (content === null || !props.project || !canSend || submitting.current) return;
    pendingWorkContent.current = null;
    setWorkPermissionPromptOpen(false);
    if (!props.session) setDraftPermissionMode(mode);
    await sendDraft(content, mode);
  }
  async function permission(requestId: string, approved: boolean) {
    setPendingPermission(requestId);
    setLocalError(null);
    try {
      await props.onPermission(requestId, approved);
    } catch (reason) {
      setLocalError(errorMessage(reason));
    } finally {
      setPendingPermission((current) => (current === requestId ? null : current));
    }
  }
  function suggestion(value: string) {
    setDraft(value);
    textarea.current?.focus();
  }

  return (
    <div className={`chat-surface ${hasMessages ? 'has-messages' : ''}`}>
      <Modal
        open={workPermissionPromptOpen}
        onOpenChange={(open) => {
          if (!open) cancelWorkPrompt();
        }}
        title="确认工作权限"
        description="当前权限为「逐次询问」，工作过程中可能频繁打断你。"
        initialFocusRef={cancelWorkPermission}
        returnFocusRef={textarea}
      >
        <div className="work-permission-confirmation">
          <p>
            文件读写、命令执行等工具操作可能反复请求确认。你可以保持逐次询问，也可以切换到完全授权。
          </p>
          <p>完全授权会自动批准此工作中的所有工具请求。</p>
          <div className="work-permission-actions">
            <button
              className="button"
              type="button"
              ref={cancelWorkPermission}
              onClick={cancelWorkPrompt}
            >
              取消
            </button>
            <button
              className="button"
              type="button"
              disabled={!canSend || !props.project}
              onClick={() => {
                void confirmWorkPermission('ask');
              }}
            >
              确认并继续
            </button>
            <button
              className="button primary"
              type="button"
              disabled={!canSend || !props.project}
              onClick={() => {
                void confirmWorkPermission('allow-all');
              }}
            >
              切换到完全授权并继续
            </button>
          </div>
        </div>
      </Modal>
      <div
        className="transcript-scroll"
        ref={transcript}
        onScroll={() => {
          const element = transcript.current;
          if (element) {
            stickToBottom.current =
              element.scrollHeight - element.scrollTop - element.clientHeight < 100;
            setJumpVisible(!stickToBottom.current);
          }
        }}
      >
        {!hasMessages && !props.historyLoading ? (
          <div className="welcome">
            <div className="welcome-eyebrow">
              <span className="welcome-emblem">
                <Sparkles size={20} strokeWidth={1.5} />
              </span>
              {props.project ? '把想法，带进项目。' : '给想法一点空间。'}
            </div>
            <h1>{props.project ? '今天，一起构建什么？' : '从一个想法，\n到下一步。'}</h1>
            <p>
              {props.project ? (
                <>
                  Agent 已准备好在 <strong>{props.project.name}</strong> 中和你一起工作。
                </>
              ) : (
                '理清思路，探索问题，找到新的可能。'
              )}
            </p>
            {props.project && (
              <div className="project-location">
                {props.project.connection.kind === 'ssh' ? (
                  <Server size={14} />
                ) : (
                  <Folder size={14} />
                )}
                <span>
                  {props.project.connection.kind === 'ssh'
                    ? `${props.project.connection.user}@${props.project.connection.host}:`
                    : ''}
                  {props.project.path}
                </span>
              </div>
            )}
            <div className="suggestions">
              {(props.project
                ? [
                    {
                      icon: Compass,
                      title: '认识这个项目',
                      description: '梳理结构与关键模块',
                      prompt:
                        '请先查看这个项目，概述它的结构、技术栈和关键模块，然后告诉我适合从哪里开始。',
                    },
                    {
                      icon: Code2,
                      title: '一起实现功能',
                      description: '把一个想法变成代码',
                      prompt: '我想在这个项目中实现一个新功能：',
                    },
                    {
                      icon: Wrench,
                      title: '检查与改进',
                      description: '找到值得解决的问题',
                      prompt:
                        '请检查这个项目，找出影响正确性或可维护性的具体问题，说明原因和改进建议。',
                    },
                  ]
                : [
                    {
                      icon: Lightbulb,
                      title: '整理一个想法',
                      description: '从模糊到清晰',
                      prompt: '帮我一起梳理一个想法：',
                    },
                    {
                      icon: Compass,
                      title: '探索一个问题',
                      description: '换个角度看世界',
                      prompt: '我想深入了解一个问题：',
                    },
                    {
                      icon: FileText,
                      title: '写点有用的东西',
                      description: '让表达恰到好处',
                      prompt: '帮我写一份简洁、自然的',
                    },
                  ]
              ).map(({ icon: Icon, title, description, prompt }) => (
                <button className="suggestion" key={title} onClick={() => suggestion(prompt)}>
                  <Icon size={19} strokeWidth={1.5} />
                  <strong>{title}</strong>
                  <span>{description}</span>
                  <ArrowRight size={14} className="suggestion-arrow" />
                </button>
              ))}
            </div>
          </div>
        ) : (
          <div className="transcript" role="log" aria-label="对话消息" aria-live="polite">
            {props.historyLoading && (
              <div className="history-loading">
                <Spinner />
                正在载入对话…
              </div>
            )}
            {props.conversation.messages.map((message) =>
              message.role === 'user' ? (
                <div className="user-message" key={message.id}>
                  <div>{message.content}</div>
                </div>
              ) : message.role === 'assistant' ? (
                <article className="assistant-message" key={message.id}>
                  <div className="assistant-label">
                    <span className="mini-logo">r</span>Relay
                  </div>
                  <Markdown content={message.content} />
                  {message.interrupted && <span className="interrupted-label">输出已停止</span>}
                  <CopyButton text={message.content} />
                </article>
              ) : (
                <details className="tool-message" key={message.id}>
                  <summary>
                    <Terminal size={14} />
                    {message.role === 'tool' ? '工具输出' : '会话信息'}
                  </summary>
                  <pre>{message.content}</pre>
                </details>
              ),
            )}
            {props.conversation.stream && (
              <article className="assistant-message streaming">
                <div className="assistant-label">
                  <span className="mini-logo">r</span>Relay
                </div>
                <Markdown content={props.conversation.stream} />
              </article>
            )}
            {props.conversation.tools.length > 0 && (
              <div className="tool-activities">
                {props.conversation.tools.map((tool, index) => (
                  <details
                    key={tool.toolCallId ?? `${tool.name}-${String(index)}`}
                    className="tool-activity"
                  >
                    <summary>
                      {tool.status === 'running' ? <Spinner size={13} /> : <CheckCheck size={13} />}
                      <span>{tool.name}</span>
                      <small>
                        {tool.status === 'running'
                          ? '运行中'
                          : tool.status === 'error'
                            ? '失败'
                            : '已完成'}
                      </small>
                      <ChevronDown size={13} />
                    </summary>
                    {tool.content && <pre>{tool.content}</pre>}
                  </details>
                ))}
              </div>
            )}
            {running && !props.conversation.stream && (
              <div className="thinking">
                <span />
                <span />
                <span />
                <small>{props.conversation.permissions.length ? '等待你的确认' : '正在思考'}</small>
              </div>
            )}
            {props.conversation.permissions.map((request) => (
              <div className="permission-card" key={request.requestId}>
                <div className="permission-heading">
                  <ShieldCheck size={19} />
                  <div>
                    <strong>需要你的确认</strong>
                    <span>{request.kind}</span>
                  </div>
                </div>
                <pre className="permission-description">{request.description}</pre>
                <div className="permission-actions">
                  <button
                    className="button compact"
                    disabled={pendingPermission !== null}
                    onClick={() => {
                      void permission(request.requestId, false);
                    }}
                  >
                    拒绝
                  </button>
                  <button
                    className="button primary compact"
                    disabled={pendingPermission !== null}
                    onClick={() => {
                      void permission(request.requestId, true);
                    }}
                  >
                    {pendingPermission === request.requestId ? <Spinner /> : <Check size={14} />}
                    允许这一次
                  </button>
                </div>
              </div>
            ))}
            {props.conversation.error && (
              <Notice>
                {props.conversation.error}
                {props.conversation.failedSend && (
                  <button
                    className="text-button"
                    disabled={Boolean(draft)}
                    title={draft ? '输入框已有内容，请先保留或清空它。' : undefined}
                    onClick={() => {
                      if (props.conversation.failedSend)
                        suggestion(props.conversation.failedSend.content);
                    }}
                  >
                    恢复未发送消息
                  </button>
                )}
              </Notice>
            )}
          </div>
        )}
      </div>
      <div className="composer-wrap">
        {jumpVisible && (
          <button
            className="jump-bottom icon-button"
            aria-label="跳到最新消息"
            onClick={() => {
              transcript.current?.scrollTo({
                top: transcript.current.scrollHeight,
                behavior: 'smooth',
              });
              stickToBottom.current = true;
            }}
          >
            <ArrowDown size={17} />
          </button>
        )}
        {localError && <Notice>{localError}</Notice>}
        {workHistoryUncertain && !props.historyLoading && (
          <Notice>
            <span>尚未读取此工作的历史，请重新载入后确认首条消息的工具权限。</span>
            <button
              className="text-button"
              onClick={props.onReloadHistory}
              disabled={!props.connected}
            >
              重新载入对话
            </button>
          </Notice>
        )}
        {props.modelError && (
          <Notice>
            <span>{props.modelError}</span>
            <button
              className="text-button"
              type="button"
              onClick={props.onConfirmModel}
              disabled={props.modelSaving || !props.connected}
            >
              {props.modelSaving ? (
                <>
                  <Spinner size={13} />
                  正在确认模型…
                </>
              ) : (
                '重新确认模型'
              )}
            </button>
          </Notice>
        )}
        {props.permissionModeError && (
          <Notice>
            <span>{props.permissionModeError}</span>
            <button
              className="text-button"
              type="button"
              onClick={props.onConfirmPermissionMode}
              disabled={props.permissionModeSaving || !props.connected}
            >
              {props.permissionModeSaving ? (
                <>
                  <Spinner size={13} />
                  正在确认权限…
                </>
              ) : (
                '重新确认权限'
              )}
            </button>
          </Notice>
        )}
        {!props.connected && (
          <div className="setup-banner">
            <span>
              <Spinner size={13} />
              正在重新连接本地服务…
            </span>
          </div>
        )}
        {props.providers.length === 0 ? (
          <div className="setup-banner">
            <span>
              <Sparkles size={16} />
              连接模型，开始你的第一个对话。
            </span>
            <button className="text-button" onClick={props.onSettings}>
              设置供应商
              <ArrowRight size={13} />
            </button>
          </div>
        ) : (
          props.project?.connection.kind !== 'ssh' &&
          !props.runtime?.available && (
            <div className="setup-banner">
              <span>
                <Terminal size={16} />
                {props.runtime?.message || '准备 Agent 运行环境。'}
              </span>
              <button className="text-button" disabled={props.installing} onClick={props.onInstall}>
                {props.installing ? (
                  <>
                    <Spinner />
                    正在准备…
                  </>
                ) : (
                  <>
                    准备运行环境
                    <ArrowRight size={13} />
                  </>
                )}
              </button>
            </div>
          )
        )}
        <form
          className={`composer ${running ? 'composer-running' : ''}`}
          onSubmit={(event) => {
            event.preventDefault();
            void submit();
          }}
        >
          <textarea
            ref={textarea}
            aria-label={props.project ? '描述任务' : '输入消息'}
            placeholder={props.project ? '描述你想完成的任务…' : '说说你的想法，或问任何问题…'}
            value={draft}
            onChange={(event) => setDraft(event.target.value)}
            rows={2}
            onKeyDown={(event) => {
              if (event.key === 'Enter' && !event.shiftKey && !event.nativeEvent.isComposing) {
                event.preventDefault();
                void submit();
              }
            }}
          />
          <div className="composer-toolbar">
            <div className="composer-options">
              <span
                className="scope-pill"
                title={props.project ? 'Agent 可操作项目文件和终端' : '聊天模式不会访问本机目录'}
              >
                {props.project ? <Folder size={13} /> : <MessageSquare size={13} />}
                {props.project ? '工作' : '聊天'}
              </span>
              <span className="toolbar-divider" />
              <Dropdown.Root>
                <Dropdown.Trigger
                  className="model-selector"
                  disabled={modelDisabled}
                  aria-label={`模型：${modelLabel}`}
                  aria-busy={props.modelSaving}
                  title={modelHint}
                >
                  {modelLabel}
                  {props.modelSaving ? <Spinner size={13} /> : <ChevronDown size={13} />}
                </Dropdown.Trigger>
                <Dropdown.Portal>
                  <Dropdown.Content
                    className="dropdown model-dropdown"
                    side="top"
                    align="start"
                    sideOffset={12}
                  >
                    {props.providers.length === 0 ? (
                      <Dropdown.Item className="dropdown-item" onSelect={props.onSettings}>
                        <Plus size={14} />
                        配置供应商
                      </Dropdown.Item>
                    ) : (
                      props.providers.map((provider) => (
                        <Dropdown.Group key={provider.id}>
                          <Dropdown.Label className="dropdown-label">
                            {provider.name}
                          </Dropdown.Label>
                          {provider.models.map((model) => (
                            <Dropdown.Item
                              key={model.id}
                              className="dropdown-item"
                              disabled={modelDisabled}
                              onSelect={() => {
                                if (!modelDisabled)
                                  props.onSelection({ providerId: provider.id, modelId: model.id });
                              }}
                            >
                              <span>{model.name || model.id}</span>
                              {props.selection?.providerId === provider.id &&
                                props.selection.modelId === model.id && <Check size={14} />}
                            </Dropdown.Item>
                          ))}
                        </Dropdown.Group>
                      ))
                    )}
                  </Dropdown.Content>
                </Dropdown.Portal>
              </Dropdown.Root>
              {props.project && (
                <PermissionModeSelector
                  mode={permissionMode}
                  disabled={permissionModeDisabled}
                  saving={props.permissionModeSaving}
                  uncertain={Boolean(props.permissionModeError)}
                  onChange={(mode) => {
                    if (permissionModeDisabled) return;
                    if (props.session) props.onPermissionMode(mode);
                    else setDraftPermissionMode(mode);
                  }}
                />
              )}
            </div>
            {running && !props.session ? (
              <button className="send-button" type="button" disabled aria-label="正在创建会话">
                <Spinner />
              </button>
            ) : running ? (
              <button
                className="send-button stop-button"
                type="button"
                aria-label="停止生成"
                onClick={props.onAbort}
              >
                <Square size={14} fill="currentColor" />
              </button>
            ) : (
              <button
                className="send-button"
                type="submit"
                disabled={!canSend}
                aria-label="发送消息"
              >
                <ArrowUp size={19} strokeWidth={2.4} />
              </button>
            )}
          </div>
        </form>
        <div className="composer-footnote">
          <span>
            {props.project ? (
              <>
                <ShieldCheck size={11} />
                {props.permissionModeError
                  ? '权限状态尚未确认，暂不能发送任务'
                  : permissionMode === 'allow-all'
                    ? '已完全授权：自动批准文件读写、命令等所有工具请求'
                    : '敏感操作会先征求你的确认'}
              </>
            ) : (
              <>
                <LockKeyhole size={11} />
                聊天不访问本机文件或终端
              </>
            )}
          </span>
          <span>Enter 发送 · Shift Enter 换行</span>
        </div>
      </div>
    </div>
  );
}

const MarkdownRenderer = lazy(() => import('./Markdown'));

function Markdown({ content }: { content: string }) {
  return (
    <Suspense fallback={<div className="markdown">{content}</div>}>
      <MarkdownRenderer content={content} />
    </Suspense>
  );
}

function CopyButton({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const timeout = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  useEffect(() => () => clearTimeout(timeout.current), []);
  return (
    <button
      className="icon-button copy-button"
      aria-label={copied ? '已复制' : '复制回复'}
      onClick={() => {
        void navigator.clipboard
          .writeText(text)
          .then(() => {
            setCopied(true);
            clearTimeout(timeout.current);
            timeout.current = setTimeout(() => setCopied(false), 1600);
          })
          .catch(() => setCopied(false));
      }}
    >
      {copied ? <Check size={14} /> : <Copy size={14} />}
    </button>
  );
}
