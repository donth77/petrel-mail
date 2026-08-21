import { useEffect, useState } from 'react';
import { Archive, MoreVertical, Star } from 'lucide-react';
import { api, type Thread } from '../lib/api';
import { count as fmtCount } from '../lib/format';
import { Icon } from './Icon';
import { t } from '../lib/strings';

export function Reader({ thread }: { thread: Thread | null }) {
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setUrl(null);
    if (!thread) return;
    api
      .messageUrl(thread.id)
      .then((u) => live && setUrl(u || null))
      .catch(() => live && setUrl(null));
    return () => {
      live = false;
    };
  }, [thread?.id]);

  if (!thread) {
    return (
      <section className="reader" aria-label={t('reader-none-title')}>
        <div className="empty">
          <h2>{t('reader-none-title')}</h2>
          <p>{t('reader-none-body')}</p>
        </div>
      </section>
    );
  }

  const subject = thread.subject || '(no subject)';
  return (
    <section className="reader" aria-label={subject}>
      <header className="reader-head">
        <div className="reader-headrow">
          <div className="reader-title">
            <h1 className="reader-subject">{subject}</h1>
            <div className="reader-meta">
              {thread.participants || thread.from_display || thread.from_addr}
              {thread.message_count > 1 && (
                <>
                  {' · '}
                  <span className="mono">
                    {t('reader-message-count', { count: fmtCount(thread.message_count) })}
                  </span>
                </>
              )}
            </div>
          </div>
          <div className="reader-actions">
            <button
              type="button"
              className={`act-icon${thread.starred ? ' on' : ''}`}
              aria-label={t('reader-star')}
              aria-pressed={thread.starred}
              title={`${t('reader-star')} (S)`}
            >
              <Icon icon={Star} />
            </button>
            <button
              type="button"
              className="act-icon"
              aria-label={t('reader-archive')}
              title={`${t('reader-archive')} (E)`}
            >
              <Icon icon={Archive} />
            </button>
            <button
              type="button"
              className="act-icon"
              aria-label={t('reader-more')}
              title={t('reader-more')}
            >
              <Icon icon={MoreVertical} />
            </button>
          </div>
        </div>
      </header>
      <div className="reader-body">
        {/* Mail HTML only ever renders inside the sandboxed custom-scheme frame
            (ADR-0004). Nothing here may widen that. */}
        {url && <iframe src={url} sandbox="" title={subject} />}
      </div>
    </section>
  );
}
