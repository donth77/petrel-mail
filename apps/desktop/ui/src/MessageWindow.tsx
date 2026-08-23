import { useCallback, useEffect, useState } from 'react';
import { api, type Thread } from './lib/api';
import { Reader } from './components/Reader';
import { Toast } from './components/Toast';
import { t } from './lib/strings';
import { useMessageLinks } from './lib/links';
import { useDropGuard } from './lib/useFileDrop';

/**
 * One conversation, alone in its own window.
 *
 * The reason to pop a message out is to keep it open while you do something
 * else — read the brief while you answer it, keep the address in front of you
 * while you fill in a form. So this window does the reading half in full and
 * leaves the rest behind: no rail, no list, no second sync loop.
 *
 * Triage still works, because a message you are working from is exactly the
 * kind you finish with. It applies to the same store the main window is reading
 * from; that window will show it on its next pass.
 */
export function MessageWindow({ threadId }: { threadId: number }) {
  const [thread, setThread] = useState<Thread | null>(null);
  const [toast, setToast] = useState<string | null>(null);
  const [gone, setGone] = useState(false);
  useDropGuard();

  useEffect(() => {
    let live = true;
    // Asked for by id, not looked for in a mailbox. This window knows which
    // conversation it was opened onto and nothing about where that conversation
    // lives, so scanning the inbox found only the ones that happened to be
    // there — a starred or archived message opened into "no longer here".
    api
      .threadById(threadId)
      .then((found) => {
        if (!live) return;
        setThread(found);
        setGone(found === null);
      })
      .catch((e) => live && setToast(String(e)));
    return () => {
      live = false;
    };
  }, [threadId]);

  // This window has no composer of its own, so a mail link starts a draft and
  // opens one. Web links go to the browser, the same as in the main window —
  // the policy is shared rather than reimplemented here.
  useMessageLinks(
    useCallback(async (addr: string) => {
      try {
        const id = await api.saveDraft(null, addr, '', '', '');
        await api.popoutCompose(id);
      } catch (e) {
        setToast(String(e));
      }
    }, []),
  );

  const close = async () => {
    try {
      const { getCurrentWindow } = await import('@tauri-apps/api/window');
      await getCurrentWindow().close();
    } catch {
      // Not under Tauri, or the window has gone already. Nothing to do.
    }
  };

  if (gone) {
    return (
      <div className="empty">
        <p>{t('popout-missing')}</p>
      </div>
    );
  }

  return (
    <div className="message-window">
      <Reader
        thread={thread}
        view="inbox"
        onToast={setToast}
        // Already the whole window, and already its own window, so neither
        // control is offered here at all.
        full
        onAction={(kind) => {
          void api
            .triage(threadId, kind)
            .then((receipt) => {
              setToast(receipt.description);
              // Filing it somewhere means this window is now showing something
              // that is no longer where you left it. Closing is the honest
              // outcome; leaving a stale copy open is not.
              if (kind === 'archive' || kind === 'trash' || kind === 'spam') void close();
            })
            .catch((e) => setToast(t('triage-failed', { error: String(e) })));
        }}
        // Move, tag and snooze all want a picker anchored to the main window's
        // model of folders and tags. Rather than half-build them here, they are
        // left to the window that has them.
        onMove={() => setToast(t('popout-in-main-window'))}
        onTag={() => setToast(t('popout-in-main-window'))}
        onSnooze={() => setToast(t('popout-in-main-window'))}
      />
      <Toast message={toast} onDone={() => setToast(null)} />
    </div>
  );
}
