import * as Dropdown from '@radix-ui/react-dropdown-menu';
import {
  ArrowUpRight,
  ChevronDown,
  Folder,
  MessageSquare,
  MoreHorizontal,
  PanelLeftClose,
  Plus,
  Server,
  Settings2,
  SquarePen,
  Sparkles,
  Trash2,
} from 'lucide-react';

import type { Project, Session } from '../lib/types';
import { Spinner } from './ui';

interface SidebarProps {
  sessions: Session[];
  projects: Project[];
  activeSessionId: string | null;
  activeProjectId: string | null;
  connected: boolean;
  onNewChat: () => void;
  onSession: (session: Session) => void;
  onProject: (project: Project) => void;
  onCreateProject: () => void;
  onSettings: () => void;
  onDeleteSession: (session: Session) => void;
  onRenameSession: (session: Session) => void;
  onRegenerateTitle: (session: Session) => void;
  titleGenerations: ReadonlySet<string>;
  onClose: () => void;
}

export function Sidebar(props: SidebarProps) {
  const chats = props.sessions.filter((session) => session.projectId === null);
  return (
    <aside className="sidebar" aria-label="主导航">
      <div className="sidebar-brand">
        <span className="brand-symbol">
          r<span />
        </span>
        <span>Relay</span>
        <button
          className="icon-button sidebar-collapse"
          onClick={props.onClose}
          aria-label="收起侧栏"
        >
          <PanelLeftClose size={17} />
        </button>
      </div>
      <button className="new-chat-button" onClick={props.onNewChat}>
        <SquarePen size={17} />
        新建对话<span className="shortcut">⌘ N</span>
      </button>
      <div className="sidebar-scroll">
        <section className="sidebar-section">
          <div className="section-label">
            <span>聊天</span>
            <MessageSquare size={13} />
          </div>
          {chats.length === 0 ? (
            <p className="sidebar-empty">想法，从这里开始。</p>
          ) : (
            chats.map((session) => (
              <SessionItem
                key={session.id}
                session={session}
                active={session.id === props.activeSessionId}
                onSelect={props.onSession}
                onDelete={props.onDeleteSession}
                onRename={props.onRenameSession}
                onRegenerateTitle={props.onRegenerateTitle}
                titleGenerating={props.titleGenerations.has(session.id)}
                connected={props.connected}
              />
            ))
          )}
        </section>
        <section className="sidebar-section">
          <div className="section-label">
            <span>项目</span>
            <button
              className="icon-button tiny"
              onClick={props.onCreateProject}
              aria-label="创建项目"
            >
              <Plus size={15} />
            </button>
          </div>
          {props.projects.map((project) => {
            const active = project.id === props.activeProjectId;
            const sessions = props.sessions.filter((session) => session.projectId === project.id);
            return (
              <div className="project-group" key={project.id}>
                <button
                  className={`project-nav ${active && !props.activeSessionId ? 'selected' : ''}`}
                  onClick={() => props.onProject(project)}
                  title={project.path}
                >
                  {project.connection.kind === 'ssh' ? <Server size={16} /> : <Folder size={16} />}
                  <span>{project.name}</span>
                  <ChevronDown size={13} className={active ? '' : 'chevron-closed'} />
                </button>
                {active && (
                  <div className="project-sessions">
                    {sessions.map((session) => (
                      <SessionItem
                        key={session.id}
                        session={session}
                        active={session.id === props.activeSessionId}
                        onSelect={props.onSession}
                        onDelete={props.onDeleteSession}
                        onRename={props.onRenameSession}
                        onRegenerateTitle={props.onRegenerateTitle}
                        titleGenerating={props.titleGenerations.has(session.id)}
                        connected={props.connected}
                      />
                    ))}
                    <button className="project-new" onClick={() => props.onProject(project)}>
                      <Plus size={13} />
                      新建工作
                    </button>
                  </div>
                )}
              </div>
            );
          })}
          <button className="add-project-button" onClick={props.onCreateProject}>
            <Plus size={15} />
            添加项目
          </button>
        </section>
      </div>
      <div className="sidebar-bottom">
        <div className="workspace-hint">
          <span className="hint-mark">
            <ArrowUpRight size={16} />
          </span>
          <div>
            <strong>你的模型，你的工作空间</strong>
            <small>在本地或远端，把想法变成现实。</small>
          </div>
        </div>
        <button className="settings-button" onClick={props.onSettings} aria-label="设置">
          <Settings2 size={17} />
          设置
          <span
            className={`connection-dot ${props.connected ? 'online' : ''}`}
            title={props.connected ? '服务已连接' : '服务未连接'}
          />
        </button>
      </div>
    </aside>
  );
}

function SessionItem({
  session,
  active,
  onSelect,
  onDelete,
  onRename,
  onRegenerateTitle,
  titleGenerating,
  connected,
}: {
  session: Session;
  active: boolean;
  onSelect: (session: Session) => void;
  onDelete: (session: Session) => void;
  onRename: (session: Session) => void;
  onRegenerateTitle: (session: Session) => void;
  titleGenerating: boolean;
  connected: boolean;
}) {
  return (
    <div className={`session-item ${active ? 'selected' : ''}`}>
      <button className="session-select" onClick={() => onSelect(session)} title={session.title}>
        {session.status === 'running' && <span className="activity-dot" />}
        {titleGenerating && (
          <span title="正在生成标题">
            <Spinner size={11} />
          </span>
        )}
        <span>{session.title || '新对话'}</span>
      </button>
      <Dropdown.Root>
        <Dropdown.Trigger
          className="icon-button session-menu"
          aria-label={`${session.title}的选项`}
        >
          <MoreHorizontal size={15} />
        </Dropdown.Trigger>
        <Dropdown.Portal>
          <Dropdown.Content className="dropdown" side="right" align="start" sideOffset={6}>
            <Dropdown.Item className="dropdown-item" onSelect={() => onRename(session)}>
              <SquarePen size={14} />
              重命名
            </Dropdown.Item>
            <Dropdown.Item
              className="dropdown-item"
              disabled={!connected || titleGenerating}
              onSelect={() => onRegenerateTitle(session)}
            >
              <Sparkles size={14} />
              {titleGenerating ? '正在生成标题…' : '重新生成标题'}
            </Dropdown.Item>
            <Dropdown.Item
              className="dropdown-item danger"
              disabled={session.status === 'running'}
              onSelect={() => onDelete(session)}
            >
              <Trash2 size={14} />
              删除对话
            </Dropdown.Item>
          </Dropdown.Content>
        </Dropdown.Portal>
      </Dropdown.Root>
    </div>
  );
}
