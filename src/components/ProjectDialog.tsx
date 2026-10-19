import {
  ArrowLeft,
  Check,
  ChevronRight,
  Folder,
  FolderOpen,
  Laptop,
  Plug,
  Server,
} from 'lucide-react';
import { useState } from 'react';

import { dispatch, errorMessage, isDesktop } from '../lib/api';
import type { Connection, DirectoryListing, Project } from '../lib/types';
import { Modal, Notice, Spinner } from './ui';

export function ProjectDialog({
  open,
  onClose,
  onCreated,
}: {
  open: boolean;
  onClose: () => void;
  onCreated: (project: Project) => Promise<void>;
}) {
  const [kind, setKind] = useState<'local' | 'ssh'>('local');
  const [name, setName] = useState('');
  const [path, setPath] = useState('');
  const [host, setHost] = useState('');
  const [user, setUser] = useState('');
  const [port, setPort] = useState('22');
  const [identityFile, setIdentityFile] = useState('');
  const [busy, setBusy] = useState<'save' | 'check' | 'browse' | null>(null);
  const [notice, setNotice] = useState<{ text: string; error: boolean } | null>(null);
  const [listing, setListing] = useState<DirectoryListing | null>(null);
  const connection: Connection =
    kind === 'local'
      ? { kind }
      : {
          kind,
          host: host.trim(),
          user: user.trim(),
          port: Number(port),
          identityFile: identityFile.trim() || null,
        };
  const connectionValid =
    kind === 'local' ||
    (host.trim().length > 0 && user.trim().length > 0 && Number(port) > 0 && Number(port) <= 65535);

  async function check() {
    setBusy('check');
    setNotice(null);
    try {
      const result = await dispatch('check_connection', { connection });
      setNotice({ text: result.message, error: false });
    } catch (reason) {
      setNotice({ text: errorMessage(reason), error: true });
    } finally {
      setBusy(null);
    }
  }
  async function browse(directory?: string) {
    setBusy('browse');
    setNotice(null);
    try {
      if (kind === 'local' && isDesktop() && directory === undefined) {
        const { open: openDirectory } = await import('@tauri-apps/plugin-dialog');
        const selected = await openDirectory({
          directory: true,
          multiple: false,
          title: '选择项目目录',
        });
        if (typeof selected === 'string') {
          setPath(selected);
          if (!name) setName(selected.split(/[\\/]/u).findLast(Boolean) ?? '新项目');
        }
      } else {
        const result = await dispatch('browse_directory', {
          connection,
          path: directory ?? (path || '~'),
        });
        setListing(result);
      }
    } catch (reason) {
      setNotice({ text: errorMessage(reason), error: true });
    } finally {
      setBusy(null);
    }
  }
  async function save() {
    setBusy('save');
    setNotice(null);
    try {
      const project = await dispatch('create_project', {
        name: name.trim(),
        path: path.trim(),
        connection,
      });
      await onCreated(project);
      onClose();
    } catch (reason) {
      setNotice({ text: errorMessage(reason), error: true });
    } finally {
      setBusy(null);
    }
  }
  return (
    <Modal
      open={open}
      onOpenChange={(value) => {
        if (!value && !busy) onClose();
      }}
      title="添加项目"
      description="选择一个工作目录，让 Agent 和你一起完成任务。"
    >
      <form
        className="project-form"
        onSubmit={(event) => {
          event.preventDefault();
          void save();
        }}
      >
        <fieldset className="connection-choice" aria-label="项目位置">
          <button
            type="button"
            className={kind === 'local' ? 'active' : ''}
            onClick={() => {
              setKind('local');
              setListing(null);
              setNotice(null);
            }}
          >
            <Laptop size={20} />
            <strong>本地项目</strong>
            <span>在这台电脑上工作</span>
            {kind === 'local' && <Check size={13} className="choice-check" />}
          </button>
          <button
            type="button"
            className={kind === 'ssh' ? 'active' : ''}
            onClick={() => {
              setKind('ssh');
              setListing(null);
              setNotice(null);
            }}
          >
            <Server size={20} />
            <strong>远程项目</strong>
            <span>通过 SSH 连接服务器</span>
            {kind === 'ssh' && <Check size={13} className="choice-check" />}
          </button>
        </fieldset>
        {kind === 'ssh' && (
          <div className="ssh-fields">
            <div className="form-grid host-port">
              <label>
                主机
                <input
                  required
                  value={host}
                  onChange={(event) => setHost(event.target.value)}
                  placeholder="dev.example.com 或 SSH 别名"
                  autoComplete="off"
                />
              </label>
              <label>
                端口
                <input
                  type="number"
                  required
                  min="1"
                  max="65535"
                  value={port}
                  onChange={(event) => setPort(event.target.value)}
                />
              </label>
            </div>
            <label>
              用户名
              <input
                required
                value={user}
                onChange={(event) => setUser(event.target.value)}
                placeholder="例如：ubuntu"
                autoComplete="username"
              />
            </label>
            <details>
              <summary>SSH 密钥（可选）</summary>
              <label>
                私钥路径
                <input
                  value={identityFile}
                  onChange={(event) => setIdentityFile(event.target.value)}
                  placeholder="~/.ssh/id_ed25519"
                />
                <small>默认使用 SSH Agent 和本机 SSH 配置。不会上传私钥。</small>
              </label>
            </details>
            <button
              className="button compact"
              type="button"
              disabled={!connectionValid || busy !== null}
              onClick={() => {
                void check();
              }}
            >
              {busy === 'check' ? <Spinner /> : <Plug size={14} />}测试连接
            </button>
          </div>
        )}
        <label>
          项目名称
          <input
            value={name}
            onChange={(event) => setName(event.target.value)}
            required
            placeholder="给项目起个名字"
          />
        </label>
        <label>
          工作目录
          <div className="input-with-button">
            <input
              required
              value={path}
              onChange={(event) => setPath(event.target.value)}
              placeholder={
                kind === 'ssh' ? '/home/ubuntu/projects/my-app' : '/Users/you/projects/my-app'
              }
            />
            <button
              className="button"
              type="button"
              disabled={!connectionValid || busy !== null}
              onClick={() => {
                void browse();
              }}
              aria-label="浏览目录"
            >
              {busy === 'browse' ? <Spinner /> : <FolderOpen size={17} />}
            </button>
          </div>
          <small>
            {kind === 'ssh'
              ? '首次创建时会自动准备远端运行环境。'
              : 'Agent 可在这个目录中读取文件、编辑代码和执行命令。'}
          </small>
        </label>
        {listing && (
          <div className="directory-browser">
            <div className="directory-heading">
              <button
                type="button"
                className="icon-button"
                disabled={busy !== null}
                onClick={() => {
                  void browse(`${listing.path}/..`);
                }}
                aria-label="上级目录"
              >
                <ArrowLeft size={15} />
              </button>
              <span title={listing.path}>{listing.path}</span>
              <button
                type="button"
                className="text-button"
                onClick={() => {
                  setPath(listing.path);
                  if (!name) setName(listing.path.split('/').findLast(Boolean) ?? '新项目');
                  setListing(null);
                }}
              >
                选择此目录
              </button>
            </div>
            <div className="directory-entries">
              {listing.entries.length === 0 ? (
                <p>没有子目录，可直接选择当前目录。</p>
              ) : (
                listing.entries.map((entry) => (
                  <button
                    type="button"
                    key={entry.path}
                    disabled={busy !== null}
                    onClick={() => {
                      void browse(entry.path);
                    }}
                  >
                    <Folder size={15} />
                    <span>{entry.name}</span>
                    <ChevronRight size={14} />
                  </button>
                ))
              )}
            </div>
          </div>
        )}
        {notice && <Notice tone={notice.error ? 'error' : 'success'}>{notice.text}</Notice>}
        <div className="form-footer">
          <button className="button" type="button" onClick={onClose} disabled={busy !== null}>
            取消
          </button>
          <button
            className="button primary"
            type="submit"
            disabled={!connectionValid || busy !== null}
          >
            {busy === 'save' ? <Spinner /> : <Folder size={15} />}
            {busy === 'save' ? '正在准备项目…' : '创建项目'}
          </button>
        </div>
      </form>
    </Modal>
  );
}
