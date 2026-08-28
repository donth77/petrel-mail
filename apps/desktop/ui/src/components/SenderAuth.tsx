import { useEffect, useState } from 'react';
import { BadgeCheck, ShieldAlert } from 'lucide-react';
import { api, type AuthInfo } from '../lib/api';
import { Icon } from './Icon';
import { Tip } from './Tip';
import { t } from '../lib/strings';

/**
 * Whether the sender is who the From line says.
 *
 * Petrel does not check this itself and cannot: SPF needs the connecting IP,
 * which is gone by the time a message is stored, and DKIM needs a DNS lookup
 * against a key that may have rotated since. The server that accepted the mail
 * did the work and wrote it into `Authentication-Results`. This relays that.
 *
 * Three rules, and each exists because the obvious version of this feature
 * gets it wrong:
 *
 * 1. **Silence is the default.** Plenty of ordinary mail carries no verdict,
 *    including anything delivered by a server that does not check. A missing
 *    header must look like nothing at all, never like a warning — otherwise
 *    the mark means "my provider is thorough" and people learn to ignore it.
 *
 * 2. **Only DMARC earns the tick.** SPF and DKIM each pass for a domain that
 *    need not be the one you can see in the From line, so a forgery can be
 *    dkim=pass. DMARC is the check that ties the result to the visible
 *    address, which is the only thing worth telling somebody about.
 *
 * 3. **It says what it checked, not that the mail is safe.** "Really from
 *    example.com" is true and useful. A green tick that reads as "trustworthy"
 *    would be doing the phishing for the attacker: plenty of hostile mail is
 *    perfectly authenticated, from a domain the sender genuinely owns.
 */
export function SenderAuth({ messageId }: { messageId: number }) {
  const [info, setInfo] = useState<AuthInfo | null>(null);

  useEffect(() => {
    let live = true;
    setInfo(null);
    api
      .authenticationInfo(messageId)
      .then((a) => live && setInfo(a))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [messageId]);

  if (!info || info.verified === null) return null;

  const domain = info.domain ?? '';
  const detail = [
    info.spf && `SPF ${info.spf}`,
    info.dkim && `DKIM ${info.dkim}`,
    info.dmarc && `DMARC ${info.dmarc}`,
  ]
    .filter(Boolean)
    .join(' · ');

  if (info.verified) {
    return (
      <Tip label={`${t('auth-pass-tip', { domain })}${detail ? ` — ${detail}` : ''}`}>
        <span className="auth auth-pass" aria-label={t('auth-pass-tip', { domain })}>
          <Icon icon={BadgeCheck} size={13} />
          <span className="auth-text">{t('auth-pass')}</span>
        </span>
      </Tip>
    );
  }

  return (
    <Tip label={`${t('auth-fail-tip', { domain })}${detail ? ` — ${detail}` : ''}`}>
      <span className="auth auth-fail" aria-label={t('auth-fail-tip', { domain })}>
        <Icon icon={ShieldAlert} size={13} />
        <span className="auth-text">{t('auth-fail')}</span>
      </span>
    </Tip>
  );
}
