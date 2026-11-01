import { useEffect, useRef, useState } from 'react';
import { api } from './lib/api';
import { draftFromRecord } from './lib/draft-record';
import { Compose, type Draft } from './components/Compose';
import { firstUnsendable, hasRecipient } from './lib/recipients';
import { Picker } from './components/Picker';
import {
  ATTACHMENT_LIMIT,
  pickAttachments,
  withAttached,
  type Attached,
} from './lib/attachments';
import { useDroppedAttachments } from './lib/useDroppedAttachments';
import { settleDraft } from './lib/close-draft';
import { AUTOSAVE_MS, draftSignature, slotFor, unsaved } from './lib/draft-autosave';
import type { ComposerSlot } from './lib/draft-autosave';
import { fileSize } from './lib/format';
import { snoozeOptions } from './lib/snooze';
import { Toast } from './components/Toast';
import { t } from './lib/strings';
import { useSettings } from './lib/settings';
import { useDropGuard } from './lib/useFileDrop';

/**
 * A popped-out composer, alone in its own window.
 *
 * Deliberately not the whole app with everything else hidden: a second rail,
 * list and sync loop would cost real memory and a second poll against the mail
 * server, to show nothing. This window knows about one draft.
 *
 * Send puts the message in the outbox with the same undo window the main
 * composer gives, and closes this window. The countdown has nowhere to show
 * here, so the message waits out its window in the main window's Outbox,
 * where Undo still pulls it back.
 */
export function ComposeWindow({ draftId }: { draftId: number }) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  // For the undo window's length; the provider wraps every window.
  const { settings } = useSettings();
  useDropGuard();
  const [error, setError] = useState<string | null>(null);
  const [account, setAccount] = useState('');
  const [scheduling, setScheduling] = useState(false);
  // The close handler is registered once and outlives every render, so it
  // reads the draft through a ref rather than the closure it was made in.
  const draftRef = useRef<Draft | null>(null);
  draftRef.current = draft;
  // The same save bookkeeping the docked composer keeps: the row this
  // message lives in and what that row holds, so an autosave can tell an edit
  // from a save that only stamped the id, and a chain so a close that lands
  // while an autosave is in flight updates the same row.
  const slotRef = useRef<ComposerSlot>({ id: draftId, signature: null });
  const saveChain = useRef<Promise<number | null>>(Promise.resolve(null));

  useEffect(() => {
    let live = true;
    api
      .loadDraft(draftId)
      .then((d) => {
        if (!live) return;
        const loaded = draftFromRecord(d);
        slotRef.current = slotFor(loaded);
        setDraft(loaded);
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

  /** Closes the window without asking anything of it: what needed saving
   *  has been saved by the time this is called. `destroy` rather than
   *  `close`, which would raise the close request this window answers by
   *  saving again. */
  const destroy = async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().destroy();
    } catch {
      // Not under Tauri, or the window has gone already. Nothing useful to do.
    }
  };

  const drops = useDroppedAttachments({
    getDraft: () => draftRef.current,
    setDraft,
    stage: api.stageAttachment,
    onRejected: (names) =>
      setToast(
        t('compose-too-large', {
          name: names.join(', '),
          limit: fileSize(ATTACHMENT_LIMIT),
        }),
      ),
    onFailed: (error) => setToast(t('compose-attach-failed', { error: String(error) })),
  });

  /** Writes the draft and remembers its row. Throws on failure, so every
   *  chain that would go on to close the window stops here instead. */
  const save = (d: Draft): Promise<number | null> => {
    const slot = slotRef.current;
    const run = saveChain.current.then(async () => {
      const ready = await drops.finish(d);
      const signature = draftSignature(ready);
      const known = slot.id ?? ready.savedId ?? null;
      const id = await api.saveDraft(known, ready.to, ready.subject, ready.body, ready.html, {
        cc: ready.cc,
        bcc: ready.bcc ?? '',
        inReplyTo: ready.inReplyTo ?? null,
        references: ready.references ?? [],
        attachments: (ready.attachments ?? []).map((a) => a.path),
      });
      slot.id = id;
      slot.signature = signature;
      setDraft((cur) => (cur ? { ...cur, savedId: id } : cur));
      return id;
    });
    // The chain must not stay rejected, or every later save would skip.
    saveChain.current = run.catch(() => null);
    return run;
  };
  const saveRef = useRef(save);
  saveRef.current = save;

  // Drafts save as they are typed, exactly as in the docked composer. This
  // window had no autosave at all: a native close lost everything since the
  // last deliberate save, which was usually everything.
  useEffect(() => {
    if (!draft || !unsaved(draft, slotRef.current)) return;
    const timer = window.setTimeout(() => {
      void saveRef.current(draft).catch(() => {});
    }, AUTOSAVE_MS);
    return () => window.clearTimeout(timer);
  }, [draft]);

  /** Puts the message away and says whether the window may go. A save that
   *  fails keeps the window, with the reason on screen. */
  const settle = async (): Promise<boolean> => {
    const d = draftRef.current;
    if (!d) return true;
    const result = await settleDraft(d, slotRef.current, saveRef.current, api.pushDraft);
    if (!result.ok) setToast(t('compose-save-failed', { error: result.error }));
    return result.ok;
  };
  const settleRef = useRef(settle);
  settleRef.current = settle;

  // The window's own close button, and ⌘W, arrive here rather than simply
  // closing: the message is written first, and a window whose message could
  // not be written stays open. The runtime destroys the window once the
  // handler returns unless it was told not to.
  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | null = null;
    void import('@tauri-apps/api/window')
      .then(({ getCurrentWindow }) =>
        getCurrentWindow().onCloseRequested(async (event) => {
          const ok = await settleRef.current();
          if (!ok) event.preventDefault();
        }),
      )
      .then((stop) => {
        if (live) unlisten = stop;
        else stop();
      })
      .catch(() => {
        // Not under Tauri. There is no native close to intercept.
      });
    return () => {
      live = false;
      unlisten?.();
    };
  }, []);

  /** Files picked or dropped, added to the message as it is now. Written
   *  back from the copy taken when the file was dropped, a large one's
   *  seconds of staging reverted everything typed meanwhile, and autosave
   *  then wrote the older text (`withAttached`). */
  const addAttached = (added: Attached[], refused: string[]) => {
    const now = draftRef.current;
    if (!now) return;
    const rejected = [...refused, ...withAttached(now.attachments ?? [], added).rejected];
    setDraft((cur) =>
      cur ? { ...cur, attachments: withAttached(cur.attachments ?? [], added).kept } : cur,
    );
    if (rejected.length > 0) {
      setToast(
        t('compose-too-large', {
          name: rejected.join(', '),
          limit: fileSize(ATTACHMENT_LIMIT),
        }),
      );
    }
  };

  if (error) return <div className="empty"><p>{error}</p></div>;
  if (!draft) return <div className="empty" />;

  return (
    <div className="compose-window">
      <Compose
        draft={draft}
        account={account}
        onChange={(update) => setDraft((cur) => (cur ? update(cur) : cur))}
        // Closing keeps the message, exactly as the docked composer does —
        // and only closes once it is kept.
        onClose={() =>
          void settle().then((ok) => {
            if (ok) return destroy();
          })
        }
        onSaveDraft={(d) =>
          void save(d)
            .then(() => setToast(t('compose-saved')))
            .catch((e) => setToast(t('compose-save-failed', { error: String(e) })))
        }
        onNotice={setToast}
        onAttach={() => {
          const base = draft.attachments ?? [];
          void pickAttachments(base, api.attachmentInfo, () => api.pickFiles('attach'))
            .then((result) => {
              if (result) addAttached(result.kept.slice(base.length), result.rejected);
            })
            .catch((e) => setToast(t('compose-attach-failed', { error: String(e) })));
        }}
        onDropFiles={drops.drop}
        onRemoveCopying={drops.remove}
        onSendLater={(d) => {
          // The checks Send makes, before a time is asked for: a message
          // scheduled with nobody to send to failed later, in the Outbox.
          if (!hasRecipient(d)) {
            setToast(t('compose-no-recipient'));
            return;
          }
          const bad = firstUnsendable(d);
          if (bad != null) {
            setToast(t('compose-bad-recipient', { addr: bad }));
            return;
          }
          setScheduling(true);
        }}
        // Already in its own window: popping out again would either make a
        // second window onto the same draft or do nothing.
        onPopOut={() => setToast(t('compose-already-popped'))}
        onSend={(d) => {
          // To, Cc or Bcc: a message to blind copies alone can go.
          if (!hasRecipient(d)) {
            setToast(t('compose-no-recipient'));
            return;
          }
          const bad = firstUnsendable(d);
          if (bad != null) {
            setToast(t('compose-bad-recipient', { addr: bad }));
            return;
          }
          // Into the outbox with the same undo window the main composer
          // gives, rather than straight onto the wire. A popped-out send
          // had no window at all before this — and no retry, no outbox row
          // on failure, nothing but a toast in a window about to close.
          // The save throws when it fails, so the window stays with the
          // message still in it rather than closing over a send that never
          // reached the outbox.
          const wait = Number(settings.undoSendSeconds) || 0;
          void save(d)
            .then((id) => {
              if (id == null) throw new Error('no draft row');
              return api.scheduleSend(id, Date.now() + wait * 1000);
            })
            .then(() => destroy())
            .catch((e) => setToast(t('compose-failed', { error: String(e) })));
        }}
      />
      <Picker
        open={scheduling}
        mode="send-later"
        subject={draft.subject || null}
        options={snoozeOptions()}
        onClose={() => setScheduling(false)}
        onCreate={() => {}}
        onChoose={(at) => {
          setScheduling(false);
          if (!hasRecipient(draft)) {
            setToast(t('compose-no-recipient'));
            return;
          }
          const bad = firstUnsendable(draft);
          if (bad != null) {
            setToast(t('compose-bad-recipient', { addr: bad }));
            return;
          }
          void save(draft)
            .then((id) => {
              if (id == null) throw new Error('no draft row');
              return api.scheduleSend(id, at);
            })
            .then(() => destroy())
            .catch((e) => setToast(t('compose-save-failed', { error: String(e) })));
        }}
      />

      <Toast message={toast} onDone={() => setToast(null)} />
    </div>
  );
}
