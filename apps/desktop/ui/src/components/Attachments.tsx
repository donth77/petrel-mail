import { useState } from 'react';
import { Download, Eye, EyeOff, Paperclip } from 'lucide-react';
import { api, type Attachment } from '../lib/api';
import { Confirm } from './Confirm';
import { Icon } from './Icon';
import { fileSize } from '../lib/format';
import { t } from '../lib/strings';

/**
 * A message's attachments, as things you can actually use.
 *
 * Three verbs, chosen by what the file is. An image or a PDF previews in
 * place, inside the same sandboxed protocol that renders message bodies —
 * looking at a stranger's file must not require opening it. Everything else
 * opens in whatever the OS uses for its type, and anything that would
 * *execute* asks first, in words that say what running it means. Save is
 * always offered, because the filesystem is where attachments ultimately
 * belong.
 */
export function Attachments({
  messageId,
  attachments,
  onToast,
}: {
  messageId: number;
  attachments: Attachment[];
  onToast: (text: string) => void;
}) {
  // Which attachment is previewing, by part. One at a time: a reading pane
  // full of expanded PDFs is a filesystem, not a message.
  const [previewing, setPreviewing] = useState<number | null>(null);
  const [previewUrl, setPreviewUrl] = useState<string | null>(null);
  // The executable awaiting a decision.
  const [confirming, setConfirming] = useState<Attachment | null>(null);

  const previewable = (a: Attachment) =>
    a.mime.startsWith('image/') || a.mime === 'application/pdf';

  const togglePreview = async (a: Attachment) => {
    if (previewing === a.part) {
      setPreviewing(null);
      setPreviewUrl(null);
      return;
    }
    try {
      // A fresh URL each time: the token is one-use by design.
      const url = await api.attachmentUrl(messageId, a.part);
      setPreviewing(a.part);
      setPreviewUrl(url);
    } catch (e) {
      onToast(String(e));
    }
  };

  const open = async (a: Attachment) => {
    try {
      if (await api.attachmentIsExecutable(a.filename)) {
        setConfirming(a);
        return;
      }
      await api.openAttachment(messageId, a.part);
    } catch (e) {
      onToast(t('att-open-failed', { error: String(e) }));
    }
  };

  const save = async (a: Attachment) => {
    try {
      const path = await api.pickSavePath(a.filename, 'attachment');
      if (!path) return;
      await api.saveAttachment(messageId, a.part, path);
      onToast(t('att-saved', { name: a.filename }));
    } catch (e) {
      onToast(t('att-save-failed', { error: String(e) }));
    }
  };

  return (
    <div className="msg-attachments">
      {attachments.map((a) => (
        <span key={a.part} className="att-row">
          <button
            type="button"
            className="att"
            // The name is the verb that suits the file: preview what can be
            // looked at, open what cannot.
            onClick={() => (previewable(a) ? void togglePreview(a) : void open(a))}
            title={previewable(a) ? t('att-preview') : t('att-open')}
          >
            <Icon icon={previewable(a) ? (previewing === a.part ? EyeOff : Eye) : Paperclip} size={12} />
            {a.filename}
            <span className="mono att-size">{fileSize(a.size)}</span>
          </button>
          <button
            type="button"
            className="act-icon att-save"
            aria-label={t('att-save', { name: a.filename })}
            onClick={() => void save(a)}
          >
            <Icon icon={Download} size={13} />
          </button>
        </span>
      ))}

      {previewing !== null && previewUrl && (
        // The same opaque-origin sandbox as a message body. The route serves
        // only images and PDFs, so nothing here can script or fetch.
        <iframe
          className="att-preview"
          src={previewUrl}
          sandbox=""
          title={attachments.find((a) => a.part === previewing)?.filename ?? ''}
        />
      )}

      <Confirm
        open={confirming !== null}
        title={t('att-exec-confirm', { name: confirming?.filename ?? '' })}
        detail={t('att-exec-body')}
        confirmLabel={t('att-exec-open')}
        onClose={() => setConfirming(null)}
        onConfirm={() => {
          const a = confirming;
          setConfirming(null);
          if (!a) return;
          // Confirmed: the warning above named this file and the person
          // said open it. Without the flag the shell refuses an executable
          // outright, and the dialog would be a button that does nothing.
          void api
            .openAttachment(messageId, a.part, true)
            .catch((e) => onToast(t('att-open-failed', { error: String(e) })));
        }}
      />
    </div>
  );
}
