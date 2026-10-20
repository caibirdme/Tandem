import { Download, FileText, RotateCcw, X } from 'lucide-react';
import { useCallback, useEffect, useRef, useState } from 'react';

import { useI18n } from '../i18n';
import { dispatch, errorMessage } from '../lib/api';
import { attachmentDataUrl, attachmentSizeLabel, imageAttachment } from '../lib/attachments';
import type { DraftAttachment } from '../lib/attachments';
import type { ConversationMessage } from '../lib/conversation';
import type { Attachment, AttachmentInput } from '../lib/types';
import { Modal, Spinner } from './ui';

export function DraftAttachments({
  items,
  disabled,
  onRemove,
  onRetry,
}: {
  items: DraftAttachment[];
  disabled: boolean;
  onRemove: (id: string) => void;
  onRetry: (id: string) => void;
}) {
  const { t } = useI18n();
  if (!items.length) return null;
  return (
    <ul className="attachment-list draft-attachments" aria-label={t('chat.attachments')}>
      {items.map((item) => (
        <li className="attachment-card" key={item.id}>
          <div className="attachment-preview">
            {item.status === 'loading' ? (
              <output aria-label={t('chat.attachmentLoading')}>
                <Spinner size={20} />
              </output>
            ) : item.status === 'error' ? (
              <button
                className="icon-button"
                type="button"
                disabled={disabled}
                aria-label={t('chat.attachmentRetry', { name: item.name })}
                onClick={() => onRetry(item.id)}
              >
                <RotateCcw size={20} />
              </button>
            ) : item.input && imageAttachment(item.mimeType) ? (
              <img src={attachmentDataUrl(item.input)} alt={item.name} />
            ) : (
              <FileText size={26} strokeWidth={1.5} />
            )}
          </div>
          <span className="attachment-name" title={item.name}>
            {item.name}
          </span>
          <small className="attachment-size">{attachmentSizeLabel(item.size)}</small>
          <button
            className="attachment-remove icon-button"
            type="button"
            disabled={disabled}
            aria-label={t('chat.removeAttachment', { name: item.name })}
            onClick={() => onRemove(item.id)}
          >
            <X size={12} />
          </button>
        </li>
      ))}
    </ul>
  );
}

export function MessageAttachments({ message }: { message: ConversationMessage }) {
  const { t } = useI18n();
  if (!message.attachments.length) return null;
  return (
    <ul className="attachment-list message-attachments" aria-label={t('chat.attachments')}>
      {message.attachments.map((attachment, index) => (
        <SavedAttachment
          key={attachment.id}
          sessionId={message.sessionId}
          attachment={attachment}
          pending={message.pendingAttachments?.[index]}
        />
      ))}
    </ul>
  );
}

function SavedAttachment({
  sessionId,
  attachment,
  pending,
}: {
  sessionId: string;
  attachment: Attachment;
  pending: AttachmentInput | undefined;
}) {
  const { t } = useI18n();
  const image = imageAttachment(attachment.mimeType);
  const card = useRef<HTMLLIElement>(null);
  const [input, setInput] = useState<AttachmentInput | undefined>(pending);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [open, setOpen] = useState(false);
  const request = useRef<Promise<AttachmentInput> | null>(null);
  const generation = useRef(0);
  useEffect(
    () => () => {
      generation.current += 1;
    },
    [],
  );
  const load = useCallback(async () => {
    if (input) return input;
    const currentGeneration = generation.current;
    setLoading(true);
    setError(null);
    const pendingRequest =
      request.current ?? dispatch('get_attachment', { sessionId, attachmentId: attachment.id });
    request.current = pendingRequest;
    try {
      const result = await pendingRequest;
      if (generation.current === currentGeneration) setInput(result);
      return result;
    } catch (reason) {
      if (generation.current === currentGeneration) setError(errorMessage(reason));
      return undefined;
    } finally {
      request.current = null;
      if (generation.current === currentGeneration) setLoading(false);
    }
  }, [attachment.id, input, sessionId]);

  useEffect(() => {
    if (!image || input || !card.current) return;
    if (typeof IntersectionObserver === 'undefined') {
      let canceled = false;
      queueMicrotask(() => {
        if (!canceled) void load();
      });
      return () => {
        canceled = true;
      };
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          observer.disconnect();
          void load();
        }
      },
      { rootMargin: '120px' },
    );
    observer.observe(card.current);
    return () => observer.disconnect();
  }, [image, input, load]);

  async function download() {
    const content = await load();
    if (!content) return;
    const link = document.createElement('a');
    link.href = `data:application/octet-stream;base64,${content.data}`;
    link.download = content.name;
    link.click();
  }

  return (
    <li className="attachment-card" ref={card}>
      <button
        type="button"
        className="attachment-open"
        disabled={loading}
        aria-label={t(image ? 'chat.previewAttachment' : 'chat.downloadAttachment', {
          name: attachment.name,
        })}
        onClick={() => {
          if (image) {
            void load().then((content) => {
              if (content) setOpen(true);
            });
          } else void download();
        }}
      >
        <span className="attachment-preview">
          {loading ? (
            <Spinner size={20} />
          ) : image && input ? (
            <img src={attachmentDataUrl(input)} alt={attachment.name} loading="lazy" />
          ) : image ? (
            <FileText size={26} strokeWidth={1.5} />
          ) : (
            <Download size={22} strokeWidth={1.5} />
          )}
        </span>
        <span className="attachment-name" title={attachment.name}>
          {attachment.name}
        </span>
        <small className="attachment-size">{attachmentSizeLabel(attachment.size)}</small>
      </button>
      {error && (
        <button
          className="text-button attachment-error"
          type="button"
          title={error}
          aria-label={t('chat.attachmentRetry', { name: attachment.name })}
          onClick={() => {
            void load();
          }}
        >
          {t('common.retry')}
        </button>
      )}
      {image && input && (
        <Modal
          open={open}
          onOpenChange={setOpen}
          title={attachment.name}
          description={t('chat.imagePreview')}
        >
          <img
            className="attachment-full-image"
            src={attachmentDataUrl(input)}
            alt={attachment.name}
          />
        </Modal>
      )}
    </li>
  );
}
