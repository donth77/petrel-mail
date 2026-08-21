import { t } from '../lib/strings';

/** The petrel: a bird over two waves. Drawn inline rather than shipped as an
 *  asset so it inherits the accent colour and stays crisp at any scale. */
export function TitleBar({ synced }: { synced: string }) {
  return (
    <div className="titlebar">
      <div className="wordmark">
        <svg width="20" height="14" viewBox="0 0 44 30" role="img" aria-label={t('app-name')}>
          <path
            d="M3 20 Q12 6 22 17 Q32 6 41 20"
            fill="none"
            stroke="var(--accent)"
            strokeWidth="2.8"
            strokeLinecap="round"
          />
          <path
            d="M14 24 Q22 18 30 24"
            fill="none"
            stroke="var(--accent)"
            strokeWidth="1.7"
            strokeLinecap="round"
            opacity="0.5"
          />
        </svg>
        <span>{t('app-name')}</span>
      </div>
      <span className="sync">{synced}</span>
    </div>
  );
}
