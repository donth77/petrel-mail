import { useEffect, useState } from 'react';
import { api } from './lib/api';
import { Compose, addresses, type Draft } from './components/Compose';
import { Picker } from './components/Picker';
import { ATTACHMENT_LIMIT, pickAttachments } from './lib/attachments';
import { fileSize } from './lib/format';
import { snoozeOptions } from './lib/snooze';
import { Toast } from './components/Toast';
import { t } from './lib/strings';

/**
 * A popped-out composer, alone in its own window.
 *
 * Deliberately not the whole app with everything else hidden: a second rail,
 * list and sync loop would cost real memory and a second poll against the mail
 * server, to show nothing. This window knows about one draft.
 *
 * It has no undo-send window either. That countdown exists so a message can be
 * caught in the seconds after ⌘↵, and it is watched in the window you are
 * looking at — here, pressing send closes this window, so there would be
 * nowhere for the countdown to live. Send from a popped-out composer is
 * immediate, and says so.
 */
export function ComposeWindow({ draftId }: { draftId: number }) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [account, setAccount] = useState('');
  const [scheduling, setScheduling] = useState(false);

  useEffect(() => {
    let live = true;
    api
      .loadDraft(draftId)
      .then((d) => {
        if (!live) return;
        setDraft({ to: d.to, cc: '', subject: d.subject, body: d.body, html: d.html, savedId: d.id });
      })
      .catch((e) => live && setError(String(e)));
    api
      .identity()
      .then((i) => live && setAccount(i.address))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [draftId]);

  const close = async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().close();
    } catch {
      // Not under Tauri, or the window has gone already. Nothing useful to do.
    }
  };

  const save = async (d: Draft) => {
    try {
      const id = await api.saveDraft(d.savedId ?? null, d.to, d.subject, d.body, d.html);
      setDraft({ ...d, savedId: id });
      return id;
    } catch (e) {
      setToast(t('compose-save-failed', { error: String(e) }));
      return null;
    }
  };

  if (error) return <div className="empty"><p>{error}</p></div>;
  if (!draft) return <div className="empty" />;

  return (
    <div className="compose-window">
      <Compose
        draft={draft}
        account={account}
        onChange={setDraft}
        // Closing keeps the message, exactly as the docked composer does.
        onClose={() => void save(draft).then(close)}
        onSaveDraft={() => void save(draft).then(() => setToast(t('compose-saved')))}
        onAttach={() => {
          void pickAttachments(draft.attachments ?? [], api.attachmentInfo)
            .then((result) => {
              if (!result) return;
              setDraft({ ...draft, attachments: result.kept });
              if (result.rejected.length > 0) {
                setToast(
                  t('compose-too-large', {
                    name: result.rejected.join(', '),
                    limit: fileSize(ATTACHMENT_LIMIT),
                  }),
                );
              }
            })
            .catch((e) => setToast(t('compose-attach-failed', { error: String(e) })));
        }}
        onSendLater={() => setScheduling(true)}
        // Already in its own window: popping out again would either make a
        // second window onto the same draft or do nothing.
        onPopOut={() => setToast(t('compose-already-popped'))}
        onSend={() => {
          if (addresses(draft.to).length === 0) {
            setToast(t('compose-no-recipient'));
            return;
          }
          void api
            .send(
              addresses(draft.to),
              addresses(draft.cc),
              draft.subject,
              draft.body,
              draft.html || null,
              draft.inReplyTo ?? null,
              draft.references ?? [],
              (draft.attachments ?? []).map((a) => a.path),
            )
            .then(() => {
              if (draft.savedId != null) void api.deleteDraft(draft.savedId).catch(() => {});
              return close();
            })
            .catch((e) => setToast(t('compose-failed', { error: String(e) })));
        }}
      />
      <Picker
        open={scheduling}
        mode="snooze"
        subject={draft.subject || null}
        options={snoozeOptions()}
        onClose={() => setScheduling(false)}
        onCreate={() => {}}
        onChoose={(at) => {
          setScheduling(false);
          void save(draft)
            .then((id) => (id == null ? null : api.scheduleSend(id, at)))
            .then(() => close())
            .catch((e) => setToast(t('compose-save-failed', { error: String(e) })));
        }}
      />

      <Toast message={toast} onDone={() => setToast(null)} />
    </div>
  );
}
