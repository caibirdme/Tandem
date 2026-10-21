import { afterEach, describe, expect, it, vi } from 'vitest';

import { setLocale } from '../i18n';
import {
  attachmentSize,
  attachmentSizeLabel,
  attachmentMimeType,
  createAttachmentDraft,
  filesFromTransfer,
  hasFileTransfer,
  imageAttachment,
  MAX_ATTACHMENT_BYTES,
  readAttachment,
} from './attachments';
import type { AttachmentInput } from './types';

const input: AttachmentInput = { name: 'image.png', mimeType: 'image/png', data: 'aGVsbG8=' };

function file(name = 'image.png', size = 5, mimeType = 'image/png'): File {
  const value = new File(['hello'], name, { type: mimeType });
  Object.defineProperty(value, 'size', { value: size });
  return value;
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

afterEach(() => {
  vi.unstubAllGlobals();
  setLocale('en');
});

describe('clipboard and drop file extraction', () => {
  it('uses files once when a drop exposes both files and items', () => {
    const attachment = file();
    const getAsFile = vi.fn(() => attachment);
    const files = filesFromTransfer({
      files: [attachment] as unknown as FileList,
      items: [{ kind: 'file', getAsFile }] as unknown as DataTransferItemList,
    });
    expect(files).toEqual([attachment]);
    expect(getAsFile).not.toHaveBeenCalled();
  });

  it('extracts a pasted image from items without consuming ordinary clipboard text', () => {
    const attachment = file();
    const items = [
      { kind: 'string', getAsFile: () => null },
      { kind: 'file', getAsFile: () => attachment },
      { kind: 'file', getAsFile: () => null },
    ] as unknown as DataTransferItemList;
    expect(filesFromTransfer({ files: [] as unknown as FileList, items })).toEqual([attachment]);
    expect(
      filesFromTransfer({
        files: [] as unknown as FileList,
        items: [{ kind: 'string', getAsFile: () => null }] as unknown as DataTransferItemList,
      }),
    ).toEqual([]);
    expect(hasFileTransfer({ types: ['text/plain'] })).toBe(false);
    expect(hasFileTransfer({ types: ['Files', 'text/plain'] })).toBe(true);
  });

  it('reads clipboard bytes without including the data URL prefix in the transport', async () => {
    class Reader extends EventTarget {
      result: string | null = null;
      readAsDataURL() {
        this.result = 'data:image/png;base64,aGVsbG8=';
        this.dispatchEvent(new Event('load'));
      }
    }
    vi.stubGlobal('FileReader', Reader);
    await expect(readAttachment(file())).resolves.toEqual(input);
    await expect(readAttachment(file('notes', 5, ''))).resolves.toEqual({
      ...input,
      name: 'notes',
      mimeType: 'application/octet-stream',
    });
  });
});

describe('attachment draft lifecycle', () => {
  it('reserves count and byte limits before overlapping file reads complete', async () => {
    const read = deferred<AttachmentInput>();
    const reader = vi.fn(() => read.promise);
    const draft = createAttachmentDraft(reader);
    draft.add([file('one.png', MAX_ATTACHMENT_BYTES), file('two.png', MAX_ATTACHMENT_BYTES)]);
    draft.add([file('three.png', 1)]);
    expect(draft.getSnapshot().items).toHaveLength(2);
    expect(draft.getSnapshot().error).toContain('20 MiB');
    expect(reader).toHaveBeenCalledTimes(2);
    draft.remove(draft.getSnapshot().items.map((item) => item.id));
    draft.add(Array.from({ length: 10 }, (_, index) => file(`${String(index)}.png`)));
    draft.add([file('overflow.png')]);
    expect(draft.getSnapshot().items).toHaveLength(10);
    expect(draft.getSnapshot().error).toContain('10 files');
    read.resolve(input);
    await read.promise;
    expect(draft.getSnapshot().items.every((item) => item.status === 'ready')).toBe(true);
  });

  it('rejects an oversized batch without dropping existing attachments or reading rejected bytes', async () => {
    const reader = vi.fn(async () => input);
    const draft = createAttachmentDraft(reader);
    draft.add([file()]);
    await Promise.resolve();
    draft.add([file('too-large.png', MAX_ATTACHMENT_BYTES + 1), file('small.png')]);
    expect(draft.getSnapshot().items).toHaveLength(1);
    expect(draft.getSnapshot().items[0]?.input).toEqual(input);
    expect(draft.getSnapshot().error).toContain('too-large.png');
    expect(reader).toHaveBeenCalledTimes(1);
  });

  it.each(['resolve', 'reject'] as const)(
    'does not resurrect a removed file when its read later %ss',
    async (outcome) => {
      const read = deferred<AttachmentInput>();
      const draft = createAttachmentDraft(() => read.promise);
      draft.add([file()]);
      draft.remove(draft.getSnapshot().items.map((item) => item.id));
      if (outcome === 'resolve') read.resolve(input);
      else read.reject(new Error('read failed'));
      await read.promise.catch(() => undefined);
      expect(draft.getSnapshot()).toEqual({ items: [], error: null });
    },
  );

  it('retains a read failure with an actionable retry and then permits sending', async () => {
    const reader = vi.fn().mockRejectedValueOnce(new Error('failed')).mockResolvedValueOnce(input);
    const draft = createAttachmentDraft(reader);
    draft.add([file()]);
    await Promise.resolve();
    const item = draft.getSnapshot().items[0];
    expect(item?.status).toBe('error');
    expect(draft.getSnapshot().error).toContain('Retry or remove');
    draft.retry(item?.id ?? '');
    expect(draft.getSnapshot().items[0]?.status).toBe('loading');
    await Promise.resolve();
    expect(draft.getSnapshot().items[0]?.input).toEqual(input);
    expect(draft.getSnapshot().error).toBeNull();
  });

  it('restores failed-send bytes without overwriting a newer attachment draft', () => {
    const draft = createAttachmentDraft();
    draft.restore([input]);
    expect(draft.getSnapshot().items[0]).toMatchObject({ input, status: 'ready', size: 5 });
    draft.restore([{ ...input, name: 'another.png' }]);
    expect(draft.getSnapshot().items.map((item) => item.name)).toEqual(['image.png']);
    draft.remove(draft.getSnapshot().items.map((item) => item.id));
    draft.restore([{ ...input, name: 'another.png' }]);
    expect(draft.getSnapshot().items[0]?.name).toBe('another.png');
  });

  it('ignores pending reads after disposal and translates boundary errors', async () => {
    setLocale('zh-CN');
    const read = deferred<AttachmentInput>();
    const draft = createAttachmentDraft(() => read.promise);
    draft.add([file('large.png', MAX_ATTACHMENT_BYTES + 1)]);
    expect(draft.getSnapshot().error).toContain('超过单个文件');
    draft.add([file()]);
    draft.dispose();
    read.resolve(input);
    await read.promise;
    expect(draft.getSnapshot()).toEqual({ items: [], error: null });
  });
});

describe('attachment rendering inputs', () => {
  it.each([
    ['notes.txt', 'text/plain'],
    ['source.RS', 'text/plain'],
    ['readme.md', 'text/plain'],
    ['Dockerfile', 'text/plain'],
    ['image.PNG', 'image/png'],
    ['report.pdf', 'application/pdf'],
    ['archive.zip', 'application/octet-stream'],
  ])('infers %s when the browser omits its MIME type', (name, mimeType) => {
    expect(attachmentMimeType({ name, type: '' })).toBe(mimeType);
    expect(attachmentMimeType({ name, type: 'custom/type' })).toBe('custom/type');
  });
  it('shows compact sizes with translated byte labels', () => {
    expect(attachmentSizeLabel(5)).toBe('5 B');
    expect(attachmentSizeLabel(4096)).toBe('4 KiB');
    expect(attachmentSizeLabel(1.5 * 1024 * 1024)).toBe('1.5 MiB');
    setLocale('zh-CN');
    expect(attachmentSizeLabel(5)).toBe('5 字节');
  });
  it.each([
    ['', 0],
    ['YQ==', 1],
    ['YWI=', 2],
    ['YWJj', 3],
  ])('computes decoded byte size for %s', (data, size) => {
    expect(attachmentSize(data)).toBe(size);
  });

  it('previews raster images while keeping executable formats as ordinary files', () => {
    for (const mime of [
      'image/png',
      'image/jpeg',
      'image/gif',
      'image/webp',
      'image/avif',
      'image/bmp',
    ])
      expect(imageAttachment(mime)).toBe(true);
    for (const mime of [
      'text/html',
      'image/svg+xml',
      'application/pdf',
      'application/octet-stream',
    ])
      expect(imageAttachment(mime)).toBe(false);
  });
});
