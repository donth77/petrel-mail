import { useEffect, useRef, useState } from 'react';
import { api } from './lib/api';
import { draftFromRecord } from './lib/draft-record';
import { Compose, addresses, type Draft } from './components/Compose';
import { Picker } from './components/Picker';
import { ATTACHMENT_LIMIT, pickAttachments, stageDropped } from './lib/attachments';
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
 * It has no undo-send window either. That countdown exists so a message can be
 * caught in the seconds after ⌘↵, and it is watched in the window you are
 * looking at — here, pressing send closes this window, so there would be
 * nowhere for the countdown to live. Send from a popped-out composer is
 * immediate, and says so.
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

  /** Writes the draft and remembers its row. Throws on failure, so every
   *  chain that would go on to close the window stops here instead. */
  const save = (d: Draft): Promise<number | null> => {
    const signature = draftSignature(d);
    const slot = slotRef.current;
    const run = saveChain.current.then(async () => {
      const known = slot.id ?? d.savedId ?? null;
      const id = await api.saveDraft(known, d.to, d.subject, d.body, d.html, {
        cc: d.cc,
        inReplyTo: d.inReplyTo ?? null,
        references: d.references ?? [],
        attachments: (d.attachments ?? []).map((a) => a.path),
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

  if (error) return <div className="empty"><p>{error}</p></div>;
  if (!draft) return <div className="empty" />;

  return (
    <div className="compose-window">
      <Compose
        draft={draft}
        account={account}
        onChange={setDraft}
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
          void pickAttachments(draft.attachments ?? [], api.attachmentInfo, () =>
            api.pickFiles('attach'),
          )
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
        onDropFiles={(files) => {
          void stageDropped([...files], draft.attachments ?? [], api.stageAttachment)
            .then((result) => {
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
        onSend={(d) => {
          if (addresses(d.to).length === 0) {
            setToast(t('compose-no-recipient'));
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
        mode="snooze"
        subject={draft.subject || null}
        options={snoozeOptions()}
        onClose={() => setScheduling(false)}
        onCreate={() => {}}
        onChoose={(at) => {
          setScheduling(false);
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
