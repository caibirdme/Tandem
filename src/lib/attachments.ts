import { getLocale, translate as t } from '../i18n';
import type { AttachmentInput } from './types';

export const MAX_ATTACHMENTS = 10;
export const MAX_ATTACHMENT_BYTES = 10 * 1024 * 1024;
export const MAX_TOTAL_ATTACHMENT_BYTES = 20 * 1024 * 1024;

export interface DraftAttachment {
  id: string;
  name: string;
  mimeType: string;
  size: number;
  status: 'loading' | 'ready' | 'error';
  input?: AttachmentInput;
}

export interface AttachmentDraft {
  items: DraftAttachment[];
  error: string | null;
}

const inferredTypes: Readonly<Record<string, string>> = {
  png: 'image/png',
  jpg: 'image/jpeg',
  jpeg: 'image/jpeg',
  gif: 'image/gif',
  webp: 'image/webp',
  avif: 'image/avif',
  bmp: 'image/bmp',
  pdf: 'application/pdf',
};
const textExtensions = new Set([
  'txt',
  'md',
  'mdx',
  'rst',
  'csv',
  'tsv',
  'log',
  'json',
  'jsonl',
  'ndjson',
  'yaml',
  'yml',
  'toml',
  'ini',
  'conf',
  'config',
  'properties',
  'xml',
  'html',
  'htm',
  'svg',
  'css',
  'scss',
  'sass',
  'less',
  'js',
  'jsx',
  'ts',
  'tsx',
  'mjs',
  'cjs',
  'rs',
  'py',
  'c',
  'h',
  'cpp',
  'hpp',
  'cs',
  'go',
  'java',
  'kt',
  'kts',
  'swift',
  'sh',
  'bash',
  'zsh',
  'fish',
  'ps1',
  'psm1',
  'bat',
  'cmd',
  'rb',
  'php',
  'lua',
  'r',
  'sql',
  'tex',
  'gitignore',
  'gitattributes',
  'editorconfig',
  'env',
]);
const textNames = new Set(['readme', 'license', 'dockerfile', 'makefile', 'justfile']);

export function attachmentMimeType(file: Pick<File, 'name' | 'type'>): string {
  if (file.type) return file.type;
  const name = file.name.toLowerCase();
  const extension = name.includes('.') ? (name.split('.').at(-1) ?? '') : '';
  return (
    inferredTypes[extension] ??
    (textExtensions.has(extension) || textNames.has(name)
      ? 'text/plain'
      : 'application/octet-stream')
  );
}

export function imageAttachment(mimeType: string): boolean {
  return /^image\/(?:png|jpeg|gif|webp|avif|bmp)$/u.test(mimeType);
}

export function attachmentDataUrl(input: AttachmentInput): string {
  return `data:${input.mimeType};base64,${input.data}`;
}

export function attachmentSize(data: string): number {
  return Math.floor((data.length * 3) / 4) - (data.endsWith('==') ? 2 : data.endsWith('=') ? 1 : 0);
}

export function attachmentSizeLabel(size: number): string {
  const divisor = size >= 1024 * 1024 ? 1024 * 1024 : size >= 1024 ? 1024 : 1;
  return t(
    divisor === 1
      ? 'chat.attachmentBytes'
      : divisor === 1024
        ? 'chat.attachmentKiB'
        : 'chat.attachmentMiB',
    {
      size: new Intl.NumberFormat(getLocale(), { maximumFractionDigits: 1 }).format(size / divisor),
    },
  );
}

export function filesFromTransfer(transfer: Pick<DataTransfer, 'files' | 'items'>): File[] {
  const files = Array.from(transfer.files);
  if (files.length) return files;
  return Array.from(transfer.items)
    .filter((item) => item.kind === 'file')
    .map((item) => item.getAsFile())
    .filter((file): file is File => file !== null);
}

export function hasFileTransfer(transfer: Pick<DataTransfer, 'types'>): boolean {
  return Array.from(transfer.types).includes('Files');
}

export async function readAttachment(file: File): Promise<AttachmentInput> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    const fail = () => reject(new Error(t('chat.attachmentReadFailed', { name: file.name })));
    reader.addEventListener('error', fail);
    reader.addEventListener('abort', fail);
    reader.addEventListener('load', () => {
      if (typeof reader.result !== 'string') {
        reject(new Error(t('chat.attachmentReadFailed', { name: file.name })));
        return;
      }
      resolve({
        name: file.name,
        mimeType: attachmentMimeType(file),
        data: reader.result.slice(reader.result.indexOf(',') + 1),
      });
    });
    reader.readAsDataURL(file);
  });
}

/** Reserve limits synchronously so overlapping paste and drop reads cannot exceed them. */
export function createAttachmentDraft(read = readAttachment) {
  let state: AttachmentDraft = { items: [], error: null };
  const listeners = new Set<() => void>();
  const files = new Map<string, File>();
  let disposed = false;

  function update(next: AttachmentDraft) {
    if (disposed) return;
    state = next;
    for (const listener of listeners) listener();
  }
  async function load(id: string, file: File) {
    try {
      const input = await read(file);
      if (!state.items.some((item) => item.id === id)) return;
      update({
        ...state,
        items: state.items.map((item) =>
          item.id === id ? { ...item, input, status: 'ready' } : item,
        ),
      });
    } catch {
      if (!state.items.some((item) => item.id === id)) return;
      update({
        error: t('chat.attachmentReadFailed', { name: file.name }),
        items: state.items.map((item) => (item.id === id ? { ...item, status: 'error' } : item)),
      });
    }
  }
  return {
    getSnapshot: () => state,
    subscribe(listener: () => void) {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    add(incoming: File[]) {
      if (!incoming.length || disposed) return;
      const oversized = incoming.find((file) => file.size > MAX_ATTACHMENT_BYTES);
      const total = [...state.items, ...incoming].reduce((size, item) => size + item.size, 0);
      const error =
        state.items.length + incoming.length > MAX_ATTACHMENTS
          ? t('chat.attachmentCountLimit')
          : oversized
            ? t('chat.attachmentFileLimit', { name: oversized.name })
            : total > MAX_TOTAL_ATTACHMENT_BYTES
              ? t('chat.attachmentTotalLimit')
              : null;
      if (error) {
        update({ ...state, error });
        return;
      }
      const items: DraftAttachment[] = incoming.map((file) => ({
        id: crypto.randomUUID(),
        name: file.name,
        mimeType: attachmentMimeType(file),
        size: file.size,
        status: 'loading',
      }));
      for (const [index, item] of items.entries()) {
        const file = incoming[index];
        if (file) files.set(item.id, file);
      }
      update({ items: [...state.items, ...items], error: null });
      for (const item of items) {
        const file = files.get(item.id);
        if (file) void load(item.id, file);
      }
    },
    remove(ids: readonly string[]) {
      for (const id of ids) files.delete(id);
      update({ items: state.items.filter((item) => !ids.includes(item.id)), error: null });
    },
    retry(id: string) {
      const file = files.get(id);
      if (!file || state.items.find((item) => item.id === id)?.status !== 'error') return;
      update({
        error: null,
        items: state.items.map((item) => (item.id === id ? { ...item, status: 'loading' } : item)),
      });
      void load(id, file);
    },
    restore(inputs: AttachmentInput[]) {
      if (state.items.length || disposed) return;
      update({
        error: null,
        items: inputs.map((input) => ({
          id: crypto.randomUUID(),
          name: input.name,
          mimeType: input.mimeType,
          size: attachmentSize(input.data),
          status: 'ready',
          input,
        })),
      });
    },
    dispose() {
      disposed = true;
      files.clear();
      listeners.clear();
      state = { items: [], error: null };
    },
  };
}
