import { useEffect, useState } from 'react';
import { CalendarDays, Check, HelpCircle, X } from 'lucide-react';
import { api, type InvitationTime, type InvitationView } from '../lib/api';
import { Icon } from './Icon';
import { t } from '../lib/strings';

/** "Wed, Sep 2, 14:00–15:00" — as honestly as the invitation said it. */
function timeText(start: InvitationTime | null, end: InvitationTime | null): string | null {
  if (!start) return null;
  if (start.kind === 'date') {
    const d = start.date;
    const day = new Date(`${d.slice(0, 4)}-${d.slice(4, 6)}-${d.slice(6, 8)}T12:00:00`);
    return t('invite-all-day', {
      date: day.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' }),
    });
  }
  if (start.kind === 'utc') {
    const s = new Date(start.ms);
    const date = s.toLocaleDateString(undefined, { weekday: 'short', month: 'short', day: 'numeric', year: 'numeric' });
    const from = s.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
    if (end && end.kind === 'utc') {
      const e = new Date(end.ms);
      const sameDay = s.toDateString() === e.toDateString();
      const to = e.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' });
      return sameDay
        ? `${date}, ${from}–${to}`
        : `${date}, ${from} → ${e.toLocaleDateString(undefined, { month: 'short', day: 'numeric' })}, ${to}`;
    }
    return `${date}, ${from}`;
  }
  // A wall-clock time in a named zone: shown as written, beside its zone —
  // converting it without a timezone database would be inventing a clock.
  const raw = start.raw;
  const pretty = `${raw.slice(6, 8)}.${raw.slice(4, 6)}.${raw.slice(0, 4)} ${raw.slice(9, 11)}:${raw.slice(11, 13)}`;
  const endPretty =
    end && end.kind === 'local' ? `–${end.raw.slice(9, 11)}:${end.raw.slice(11, 13)}` : '';
  return `${pretty}${endPretty}${start.tzid ? ` (${start.tzid})` : ''}`;
}

/**
 * The card an invitation renders as, above the message body.
 *
 * A REQUEST addressed to this account offers Accept, Tentative and Decline;
 * the answer travels as METHOD:REPLY through the outbox like any mail. A
 * CANCEL renders as a cancellation notice, and a plain event attachment —
 * no METHOD, nobody to answer — shows its facts and no buttons.
 */
export function InvitationCard({
  messageId,
  onToast,
}: {
  messageId: number;
  onToast: (text: string) => void;
}) {
  const [inv, setInv] = useState<InvitationView | null>(null);
  const [answered, setAnswered] = useState<string | null>(null);
  const [changing, setChanging] = useState(false);
  const [busy, setBusy] = useState(false);
  useEffect(() => {
    let live = true;
    api
      .invitation(messageId)
      .then((v) => {
        if (live) {
          setInv(v);
          // Petrel's own record first; failing that, the wire's word — an
          // invitation can arrive already answered, accepted from another
          // client before this one ever saw it.
          const fromWire =
            v?.my_partstat === 'ACCEPTED'
              ? 'accepted'
              : v?.my_partstat === 'TENTATIVE'
                ? 'tentative'
                : v?.my_partstat === 'DECLINED'
                  ? 'declined'
                  : null;
          setAnswered(v?.responded ?? fromWire);
        }
      })
      .catch(() => {});
    return () => {
      live = false;
    };
  }, [messageId]);
  if (!inv) return null;

  const cancelled = inv.method === 'CANCEL' || inv.status === 'CANCELLED';
  const when = timeText(inv.start, inv.end);
  const answer = (response: string) => {
    setBusy(true);
    void api
      .respondInvitation(messageId, response)
      .then(() => {
        setAnswered(response);
        setChanging(false);
        onToast(t('invite-answered', { response: t(`invite-${response}` as never) }));
      })
      .catch((e) => onToast(t('invite-failed', { error: String(e) })))
      .finally(() => setBusy(false));
  };

  return (
    <div className={`invite${cancelled ? ' cancelled' : ''}`} data-invitation>
      <div className="invite-head">
        <Icon icon={CalendarDays} size={15} />
        <span className="invite-title clip">{inv.summary ?? t('invite-untitled')}</span>
        {inv.recurring && <span className="invite-repeats">{t('invite-repeats')}</span>}
      </div>
      {cancelled && <div className="invite-cancelled">{t('invite-cancelled')}</div>}
      <dl className="invite-facts">
        {when && (
          <>
            <dt>{t('invite-when')}</dt>
            <dd>{when}</dd>
          </>
        )}
        {inv.location && (
          <>
            <dt>{t('invite-where')}</dt>
            <dd className="clip">{inv.location}</dd>
          </>
        )}
        {inv.organizer_email && (
          <>
            <dt>{t('invite-organizer')}</dt>
            <dd className="clip">{inv.organizer_name ?? inv.organizer_email}</dd>
          </>
        )}
      </dl>
      {!cancelled && inv.can_respond && (
        <div className="invite-acts" role="group" aria-label={t('invite-respond')}>
          {answered && !changing ? (
            <>
              <span className={`invite-answer ${answered}`}>
                <Icon icon={answered === 'declined' ? X : Check} size={13} />
                {t(`invite-${answered}` as never)}
              </span>
              {/* Minds change; a later REPLY simply supersedes the last. */}
              <button type="button" className="reply invite-change" onClick={() => setChanging(true)}>
                {t('invite-change')}
              </button>
            </>
          ) : (
            <>
              <button type="button" className="reply primary" disabled={busy} onClick={() => answer('accepted')}>
                <Icon icon={Check} size={13} /> {t('invite-accept')}
              </button>
              <button type="button" className="reply" disabled={busy} onClick={() => answer('tentative')}>
                <Icon icon={HelpCircle} size={13} /> {t('invite-tentative')}
              </button>
              <button type="button" className="reply" disabled={busy} onClick={() => answer('declined')}>
                <Icon icon={X} size={13} /> {t('invite-decline')}
              </button>
            </>
          )}
        </div>
      )}
    </div>
  );
}
