import { useEffect, useState } from 'react';
import { api, type Thread } from './lib/api';
import { Reader } from './components/Reader';
import { Toast } from './components/Toast';
import { t } from './lib/strings';

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

  useEffect(() => {
    let live = true;
    // Found by asking for the conversation rather than the message: the list
    // row is what the reader renders, and it is keyed by thread.
    api
      .threads('inbox', 0, 500)
      .then((rows) => {
        if (!live) return;
        const found = rows.find((r) => r.thread_id === threadId) ?? null;
        setThread(found);
        setGone(found === null);
      })
      .catch((e) => live && setToast(String(e)));
    return () => {
      live = false;
    };
  }, [threadId]);

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
