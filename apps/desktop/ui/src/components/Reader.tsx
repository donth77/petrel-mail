import { useEffect, useState } from 'react';
import { api, type Listing } from '../lib/api';
import { fullTime } from '../lib/format';
import { Archive, Clock } from 'lucide-react';
import { Icon } from './Icon';
import { t } from '../lib/strings';

export function Reader({ message }: { message: Listing | null }) {
  const [url, setUrl] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setUrl(null);
    if (!message) return;
    api
      .messageUrl(message.id)
      .then((u) => live && setUrl(u))
      .catch(() => live && setUrl(null));
    return () => {
      live = false;
    };
  }, [message?.id]);

  if (!message) {
    return (
      <section className="reader" aria-label={t('reader-none-title')}>
        <div className="empty">
          <h2>{t('reader-none-title')}</h2>
          <p>{t('reader-none-body')}</p>
        </div>
      </section>
    );
  }

  return (
    <section className="reader" aria-label={message.subject || '(no subject)'}>
      <header className="reader-head">
        <h1 className="reader-subject">{message.subject || '(no subject)'}</h1>
        <div className="reader-meta">
          <span>{message.from_display || message.from_addr}</span>
          <span className="mono">{fullTime(message.date_ms)}</span>
          <span style={{ flexGrow: 1 }} />
          <button type="button" className="act">
<Icon icon={Archive} size={13} />
            {t('reader-archive')} <span className="kbd">E</span>
          </button>
          <button type="button" className="act">
<Icon icon={Clock} size={13} />
            {t('reader-snooze')} <span className="kbd">B</span>
          </button>
        </div>
      </header>
      <div className="reader-body">
        {/* Mail HTML only ever renders inside the sandboxed custom-scheme frame
            (ADR-0004). Nothing here may widen that. */}
        {url && <iframe src={url} sandbox="" title={message.subject || '(no subject)'} />}
      </div>
    </section>
  );
}
