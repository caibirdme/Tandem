import * as Dropdown from '@radix-ui/react-dropdown-menu';
import {
  ChevronRight,
  Folder,
  MoreHorizontal,
  PanelLeft,
  Plus,
  RefreshCw,
  Server,
  Settings2,
  SquarePen,
  Sparkles,
  Trash2,
  X,
} from 'lucide-react';
import { lazy, Suspense, useCallback, useEffect, useRef, useState } from 'react';

import { Chat } from './components/Chat';
import { Sidebar } from './components/Sidebar';
import { Modal, Notice, Spinner } from './components/ui';
import { useRelay } from './hooks/useRelay';
import { dispatch, errorMessage } from './lib/api';
import { newSessionPayload } from './lib/sessionOptions';
import type { ModelSelection, PermissionMode, Project, Session } from './lib/types';

const Settings = lazy(async () => ({ default: (await import('./components/Settings')).Settings }));
const ProjectDialog = lazy(async () => ({
  default: (await import('./components/ProjectDialog')).ProjectDialog,
}));

export function App() {
  const relay = useRelay();
  const [projectId, setProjectId] = useState<string | null>(null);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [projectOpen, setProjectOpen] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(() => window.innerWidth >= 800);
  const [selection, setSelection] = useState<ModelSelection | null>(null);
  const [sendingScopes, setSendingScopes] = useState<ReadonlySet<string>>(new Set());
  // Explicit navigation resets the view; creating a session from its draft keeps it mounted.
  const [viewKey, setViewKey] = useState(() => crypto.randomUUID());
  const navigationRevision = useRef(0);
  const [installing, setInstalling] = useState(false);
  const [actionBusy, setActionBusy] = useState(false);
  const [sessionAction, setSessionAction] = useState<{
    session: Session;
    action: 'rename' | 'delete';
  } | null>(null);
  const [deleteProject, setDeleteProject] = useState<Project | null>(null);
  const [sessionTitle, setSessionTitle] = useState('');
  const snapshot = relay.snapshot;
  const session = snapshot?.sessions.find((item) => item.id === relay.activeSessionId);
  const project = snapshot?.projects.find((item) => item.id === (session?.projectId ?? projectId));
  const sendScope = session?.id ?? viewKey;
  const sending = sendingScopes.has(sendScope);
  const providers = snapshot?.settings.providers ?? [];
  const firstProvider = providers.find((provider) => provider.models.length > 0);
  const defaultModel = firstProvider?.models[0];
  const effectiveSelection = session
    ? { providerId: session.providerId, modelId: session.modelId }
    : selection &&
        providers.some(
          (provider) =>
            provider.id === selection.providerId &&
            provider.models.some((model) => model.id === selection.modelId),
        )
      ? selection
      : firstProvider && defaultModel
        ? { providerId: firstProvider.id, modelId: defaultModel.id }
        : null;
  const { setActiveSessionId } = relay;
  const newDraft = useCallback(
    (targetProjectId: string | null) => {
      navigationRevision.current += 1;
      setViewKey(crypto.randomUUID());
      setActiveSessionId(null);
      setProjectId(targetProjectId);
    },
    [setActiveSessionId],
  );
  const newChat = useCallback(() => newDraft(null), [newDraft]);

  useEffect(() => {
    function keydown(event: KeyboardEvent) {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'n') {
        event.preventDefault();
        newChat();
      }
      if ((event.metaKey || event.ctrlKey) && event.key === ',') {
        event.preventDefault();
        setSettingsOpen(true);
      }
    }
    window.addEventListener('keydown', keydown);
    return () => window.removeEventListener('keydown', keydown);
  }, [newChat]);

  async function send(content: string, permissionMode: PermissionMode) {
    if (!effectiveSelection) {
      setSettingsOpen(true);
      return;
    }
    const submittedNavigation = navigationRevision.current;
    let requestScope = sendScope;
    setSendingScopes((current) => new Set([...current, sendScope]));
    try {
      let target = session;
      if (!target) {
        target = await dispatch(
          'create_session',
          newSessionPayload(project?.id ?? null, effectiveSelection, permissionMode),
        );
        const targetId = target.id;
        setSendingScopes((current) => {
          const next = new Set(current);
          next.delete(sendScope);
          next.add(targetId);
          return next;
        });
        requestScope = targetId;
        relay.addSession(target, navigationRevision.current === submittedNavigation);
      }
      if (target.projectId && target.permissionMode !== permissionMode) {
        const confirmedMode = await relay.changePermissionMode(target.id, permissionMode);
        if (confirmedMode !== permissionMode)
          throw new Error('未能确认所选工具权限，消息尚未发送。请重新确认权限后再试。');
      }
      await relay.send(target.id, content);
    } finally {
      setSendingScopes((current) => {
        const next = new Set(current);
        next.delete(requestScope);
        return next;
      });
    }
  }
  async function install() {
    setInstalling(true);
    relay.setError(null);
    try {
      await dispatch('install_runtime', {});
      await relay.reload();
    } catch (reason) {
      relay.setError(errorMessage(reason));
    } finally {
      setInstalling(false);
    }
  }
  function selectSession(value: Session) {
    if (value.id === relay.activeSessionId) return;
    navigationRevision.current += 1;
    setViewKey(crypto.randomUUID());
    relay.setActiveSessionId(value.id);
    setProjectId(value.projectId);
    if (window.innerWidth < 800) setSidebarOpen(false);
  }
  function selectProject(value: Project) {
    newDraft(value.id);
    if (window.innerWidth < 800) setSidebarOpen(false);
  }
  async function actOnSession() {
    if (!sessionAction) return;
    setActionBusy(true);
    relay.setError(null);
    try {
      if (sessionAction.action === 'rename')
        await relay.renameSession(sessionAction.session.id, sessionTitle.trim());
      else {
        await relay.deleteSession(sessionAction.session.id);
        if (relay.activeSessionId === sessionAction.session.id) newDraft(project?.id ?? null);
      }
      await relay.reload();
      setSessionAction(null);
    } catch (reason) {
      relay.setError(errorMessage(reason));
    } finally {
      setActionBusy(false);
    }
  }
  function rename(value: Session) {
    setSessionTitle(value.title);
    setSessionAction({ session: value, action: 'rename' });
  }

  return (
    <div className={`app-shell ${sidebarOpen ? '' : 'sidebar-hidden'}`}>
      {sidebarOpen && (
        <>
          <button
            className="sidebar-backdrop"
            aria-label="关闭侧栏"
            onClick={() => setSidebarOpen(false)}
          />
          <Sidebar
            sessions={snapshot?.sessions ?? []}
            projects={snapshot?.projects ?? []}
            activeSessionId={relay.activeSessionId}
            activeProjectId={project?.id ?? null}
            connected={relay.connected}
            onNewChat={newChat}
            onSession={selectSession}
            onProject={selectProject}
            onCreateProject={() => setProjectOpen(true)}
            onSettings={() => setSettingsOpen(true)}
            onDeleteSession={(value) => setSessionAction({ session: value, action: 'delete' })}
            onRenameSession={rename}
            onRegenerateTitle={(value) => {
              void relay.regenerateTitle(value.id);
            }}
            titleGenerations={relay.titleGenerations}
            onClose={() => setSidebarOpen(false)}
          />
        </>
      )}
      <main className="main-content">
        <header className="topbar" data-tauri-drag-region>
          <div className="topbar-title">
            {!sidebarOpen && (
              <button
                className="icon-button"
                aria-label="展开侧栏"
                onClick={() => setSidebarOpen(true)}
              >
                <PanelLeft size={18} />
              </button>
            )}
            {project ? (
              <>
                <span className="breadcrumb">
                  {project.connection.kind === 'ssh' ? <Server size={14} /> : <Folder size={14} />}
                  {project.name}
                </span>
                <ChevronRight size={13} />
              </>
            ) : null}
            <span className="current-session-title">
              {session?.title || (project ? '新工作' : '新对话')}
            </span>
            {session && relay.titleGenerations.has(session.id) && (
              <output className="title-progress">
                <Spinner size={12} />
                正在生成标题
              </output>
            )}
          </div>
          <div className="topbar-actions">
            {project && (
              <span className="environment-badge">
                <span />
                {project.connection.kind === 'ssh' ? 'SSH' : '本地'}
              </span>
            )}
            {session && (
              <button
                className="icon-button"
                aria-label={project ? '新建工作' : '新建对话'}
                onClick={() => newDraft(project?.id ?? null)}
              >
                <Plus size={18} />
              </button>
            )}
            <Dropdown.Root>
              <Dropdown.Trigger className="icon-button" aria-label="更多选项">
                <MoreHorizontal size={20} />
              </Dropdown.Trigger>
              <Dropdown.Portal>
                <Dropdown.Content className="dropdown" sideOffset={8} align="end">
                  <Dropdown.Item className="dropdown-item" onSelect={() => setSettingsOpen(true)}>
                    <Settings2 size={14} />
                    设置
                  </Dropdown.Item>
                  {session && (
                    <Dropdown.Item className="dropdown-item" onSelect={() => rename(session)}>
                      <SquarePen size={14} />
                      重命名对话
                    </Dropdown.Item>
                  )}
                  {session && (
                    <Dropdown.Item
                      className="dropdown-item"
                      disabled={!relay.connected || relay.titleGenerations.has(session.id)}
                      onSelect={() => {
                        void relay.regenerateTitle(session.id);
                      }}
                    >
                      <Sparkles size={14} />
                      {relay.titleGenerations.has(session.id) ? '正在生成标题…' : '重新生成标题'}
                    </Dropdown.Item>
                  )}
                  <Dropdown.Item
                    className="dropdown-item"
                    onSelect={() => {
                      void relay
                        .reload()
                        .catch((reason: unknown) => relay.setError(errorMessage(reason)));
                    }}
                  >
                    <RefreshCw size={14} />
                    刷新连接状态
                  </Dropdown.Item>
                  {project && (
                    <Dropdown.Item
                      className="dropdown-item danger"
                      onSelect={() => setDeleteProject(project)}
                    >
                      <Trash2 size={14} />
                      移除项目
                    </Dropdown.Item>
                  )}
                </Dropdown.Content>
              </Dropdown.Portal>
            </Dropdown.Root>
          </div>
        </header>
        {session && relay.titleError && (
          <div className="title-notice">
            <Notice tone="neutral">
              <span>标题生成失败：{relay.titleError}。原标题已保留。</span>
              <button
                className="text-button"
                disabled={!relay.connected || relay.titleGenerations.has(session.id)}
                onClick={() => {
                  void relay.regenerateTitle(session.id);
                }}
              >
                重新生成
              </button>
              <button
                className="icon-button"
                aria-label="关闭标题提示"
                onClick={() => relay.dismissTitleError(session.id)}
              >
                <X size={14} />
              </button>
            </Notice>
          </div>
        )}
        {relay.error && (
          <div className="global-error">
            <Notice>
              {relay.error}
              <button
                className="text-button"
                onClick={() => {
                  void relay
                    .reload()
                    .catch((reason: unknown) => relay.setError(errorMessage(reason)));
                }}
              >
                重试
                <RefreshCw size={12} />
              </button>
            </Notice>
          </div>
        )}
        {relay.loading ? (
          <div className="app-loading">
            <Spinner size={23} />
            <span>正在连接你的工作空间…</span>
          </div>
        ) : (
          <Chat
            key={viewKey}
            session={session}
            project={project}
            providers={providers}
            selection={effectiveSelection}
            onSelection={(value) => {
              if (session)
                void relay
                  .changeSessionModel(session.id, value)
                  .catch((reason: unknown) => relay.setError(errorMessage(reason)));
              else setSelection(value);
            }}
            modelSaving={relay.modelSaving}
            modelError={relay.modelError}
            onConfirmModel={() => {
              if (session)
                void relay
                  .confirmSessionModel(session.id)
                  .catch((reason: unknown) => relay.setError(errorMessage(reason)));
            }}
            conversation={relay.conversation}
            historyLoading={relay.historyLoading}
            historyLoaded={relay.historyLoaded}
            onReloadHistory={relay.reloadHistory}
            sending={sending}
            permissionModeSaving={relay.permissionModeSaving}
            permissionModeError={relay.permissionModeError}
            onPermissionMode={(mode) => {
              if (session) void relay.changePermissionMode(session.id, mode);
            }}
            onConfirmPermissionMode={() => {
              if (session) void relay.confirmPermissionMode(session.id);
            }}
            runtime={snapshot?.runtime}
            installing={installing}
            onInstall={() => {
              void install();
            }}
            onSettings={() => setSettingsOpen(true)}
            onSend={send}
            onAbort={() => {
              if (session)
                void dispatch('abort_session', { sessionId: session.id }).catch((reason: unknown) =>
                  relay.setError(errorMessage(reason)),
                );
            }}
            onPermission={async (requestId, approved) => {
              if (session) await relay.respondPermission(session.id, requestId, approved);
            }}
            connected={relay.connected}
          />
        )}
      </main>
      <Suspense fallback={null}>
        {settingsOpen && snapshot && (
          <Settings
            open={settingsOpen}
            onClose={() => setSettingsOpen(false)}
            settings={snapshot.settings}
            onSaved={relay.reload}
          />
        )}
        {projectOpen && (
          <ProjectDialog
            open={projectOpen}
            onClose={() => setProjectOpen(false)}
            onCreated={async (value) => {
              await relay.reload();
              selectProject(value);
            }}
          />
        )}
      </Suspense>
      <Modal
        open={sessionAction !== null}
        onOpenChange={(value) => {
          if (!value) setSessionAction(null);
        }}
        title={sessionAction?.action === 'rename' ? '重命名对话' : '删除对话'}
        description={
          sessionAction?.action === 'rename'
            ? '让对话更容易找到。'
            : '这会删除该对话的消息记录，此操作无法撤销。'
        }
      >
        <form
          className="simple-form"
          onSubmit={(event) => {
            event.preventDefault();
            void actOnSession();
          }}
        >
          {relay.error && <Notice>{relay.error}</Notice>}
          {sessionAction?.action === 'rename' ? (
            <label>
              对话标题
              <input
                required
                value={sessionTitle}
                onChange={(event) => setSessionTitle(event.target.value)}
              />
            </label>
          ) : (
            <p>确认删除「{sessionAction?.session.title}」？</p>
          )}
          <div className="form-footer">
            <button type="button" className="button" onClick={() => setSessionAction(null)}>
              取消
            </button>
            <button
              className={`button ${sessionAction?.action === 'delete' ? 'destructive' : 'primary'}`}
              type="submit"
              disabled={actionBusy}
            >
              {actionBusy && <Spinner />}
              {sessionAction?.action === 'rename' ? '保存' : '删除'}
            </button>
          </div>
        </form>
      </Modal>
      <Modal
        open={deleteProject !== null}
        onOpenChange={(value) => {
          if (!value) setDeleteProject(null);
        }}
        title="移除项目"
        description="移除项目及其会话记录。项目目录中的文件会保留。"
      >
        <div className="simple-form">
          {relay.error && <Notice>{relay.error}</Notice>}
          <p>确认移除「{deleteProject?.name}」？</p>
          <div className="form-footer">
            <button className="button" onClick={() => setDeleteProject(null)}>
              取消
            </button>
            <button
              className="button destructive"
              disabled={actionBusy}
              onClick={() => {
                if (!deleteProject) return;
                setActionBusy(true);
                relay.setError(null);
                void relay
                  .deleteProject(deleteProject.id)
                  .then(async () => {
                    if (project?.id === deleteProject.id) newChat();
                    await relay.reload();
                    setDeleteProject(null);
                  })
                  .catch((reason: unknown) => relay.setError(errorMessage(reason)))
                  .finally(() => setActionBusy(false));
              }}
            >
              {actionBusy && <Spinner />}移除项目
            </button>
          </div>
        </div>
      </Modal>
    </div>
  );
}
