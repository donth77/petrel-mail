import { useEffect, useState } from 'react';
import { AlertTriangle, Clock, Paperclip, WifiOff } from 'lucide-react';
import { api, type OutboxRow } from '../lib/api';
import { Icon } from './Icon';
import { t } from '../lib/strings';

/**
 * Every message Petrel is holding, and why.
 *
 * Not a list of conversations. Each row here is a message in one of five
 * states, and the row's job is to say which state in the words a person would
 * use and to offer exactly the actions that state allows. Four of the five
 * resolve themselves; the amber one cannot, and no amount of engineering makes
 * it — so it states what is unknown and hands over a choice.
 *
 * Re-read every second while open. The rows carry countdowns, and the worker
 * changes their state underneath; a view of the outbox that was true a minute
 * ago is the wrong thing to make decisions from.
 */

/** "7s", "2 min", "an hour" — the granularity a countdown is read at. */
function until(ms: number, now: number): string {
  const s = Math.max(0, Math.round((ms - now) / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min`;
  const h = Math.round(m / 60);
  return h === 1 ? '1 hour' : `${h} hours`;
}

/** A scheduled time, as the row shows it: today's show a clock, others a date. */
function at(ms: number): string {
  const d = new Date(ms);
  const sameDay = new Date().toDateString() === d.toDateString();
  return sameDay
    ? d.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })
    : d.toLocaleString(undefined, { weekday: 'short', hour: '2-digit', minute: '2-digit' });
}

/** The user-facing half of a 5xx: the code and the server's words. */
function reason(error: string | null): string {
  return (error ?? '').replace(/\s+/g, ' ').trim() || '—';
}

function Row({
  row,
  now,
  onChange,
  onDiscard,
}: {
  row: OutboxRow;
  now: number;
  onChange: () => void;
  onDiscard: (row: OutboxRow) => void;
}) {
  const [checking, setChecking] = useState<string | null>(null);
  const act = (p: Promise<unknown>) => void p.then(onChange).catch(onChange);

  // A message inside its send-time window, still pullable back, as against
  // one waiting out a retry. Both are `RetryQueued` to the store; they read
  // very differently to a person.
  const pending = row.state === 'UndoWindow' || (row.state === 'RetryQueued' && row.attempts === 0);
  const waiting = row.state === 'RetryQueued' && row.attempts > 0;
  // Nothing reached the wire: the network is not there to reach.
  const offline = waiting && /unreachable|offline|no route|dns|resolve/i.test(row.error ?? '');
  const rejected = row.state === 'FailedPermanent';
  const unknown = row.state === 'NeedsAttention';
  const noSuchUser = rejected && /^5[0-9]{2}.*(no such user|unknown user|user unknown|does not exist)/i.test(row.error ?? '');

  const tone = unknown ? 'amber' : rejected ? 'red' : waiting ? 'muted' : 'normal';

  return (
    <article className="outbox-row" data-tone={tone}>
      <header className="outbox-head">
        {unknown && <Icon icon={AlertTriangle} size={14} className="outbox-glyph" />}
        {offline && <Icon icon={WifiOff} size={14} className="outbox-glyph" />}
        {pending && <Icon icon={Clock} size={14} className="outbox-glyph" />}
        <span className="outbox-subject clip">
          {unknown && <span className="outbox-needs">{t('outbox-needs-you')} — </span>}
          {row.subject || '(no subject)'}
        </span>
      </header>
      <div className="outbox-to clip">
        {t('outbox-to', { who: row.to || '—' })}
        {row.attachments > 0 && (
          <>
            {' · '}
            <Icon icon={Paperclip} size={11} />{' '}
            {t('outbox-attachments', {
              count: row.attachments,
            })}
          </>
        )}
      </div>

      <p className="outbox-why">
        {row.state === 'Transmitting' && t('outbox-transmitting')}
        {pending &&
          (row.send_after_ms - now < 60_000
            ? t('outbox-sending-in', { when: until(row.send_after_ms, now) })
            : t('outbox-scheduled-for', { when: at(row.send_after_ms) }))}
        {waiting && offline && t('outbox-offline')}
        {waiting && !offline && t('outbox-retrying', { when: until(row.next_ms ?? now, now) })}
        {rejected &&
          t(noSuchUser ? 'outbox-rejected-user' : 'outbox-rejected', { reason: reason(row.error) })}
        {unknown && (
          <>
            {t('outbox-unknown-1')}
            <br />
            {t('outbox-unknown-2')}
          </>
        )}
        {checking && <span className="outbox-checked"> {checking}</span>}
      </p>

      <div className="outbox-acts">
        {pending && (
          <>
            <button type="button" className="reply" onClick={() => act(api.outboxEdit(row.id))}>
              {t('outbox-undo')} <span className="kbd">Z</span>
            </button>
            <button type="button" className="reply primary" onClick={() => act(api.outboxSendNow(row.id))}>
              {t('outbox-send-now')}
            </button>
          </>
        )}
        {waiting && !offline && (
          <button type="button" className="reply primary" onClick={() => act(api.outboxSendNow(row.id))}>
            {t('outbox-try-now')}
          </button>
        )}
        {(waiting || rejected) && (
          <>
            <button type="button" className="reply" onClick={() => act(api.outboxEdit(row.id))}>
              {t('outbox-edit')}
            </button>
            <button type="button" className="reply danger" onClick={() => onDiscard(row)}>
              {t('outbox-discard')}
            </button>
          </>
        )}
        {unknown && (
          <>
            <button
              type="button"
              className="reply primary"
              onClick={() =>
                void api
                  .outboxCheck(row.id)
                  .then((s) => {
                    setChecking(
                      s === 'Sent'
                        ? t('outbox-checked-sent')
                        : s === 'RetryQueued'
                          ? t('outbox-checked-absent')
                          : t('outbox-checked-unknown'),
                    );
                    onChange();
                  })
                  .catch((e) => setChecking(String(e)))
              }
            >
              {t('outbox-check-again')}
            </button>
            <button type="button" className="reply" onClick={() => act(api.outboxSendNow(row.id))}>
              {t('outbox-send-anyway')}
            </button>
            <button type="button" className="reply danger" onClick={() => onDiscard(row)}>
              {t('outbox-discard')}
            </button>
          </>
        )}
      </div>
    </article>
  );
}

export function Outbox({
  onDiscard,
  onCountChange,
}: {
  onDiscard: (row: OutboxRow) => void;
  /** Told how many need a person, so the rail can turn amber. */
  onCountChange?: (total: number, needsAttention: number) => void;
}) {
  const [rows, setRows] = useState<OutboxRow[]>([]);
  const [now, setNow] = useState(() => Date.now());
  const [tick, setTick] = useState(0);

  useEffect(() => {
    let live = true;
    api
      .outbox()
      .then((r) => {
        if (!live) return;
        setRows(r);
        onCountChange?.(r.length, r.filter((x) => x.state === 'NeedsAttention').length);
      })
      .catch((e) => api.log(`list_outbox failed: ${e}`));
    return () => {
      live = false;
    };
    // Re-read on every tick: the worker moves rows between states underneath.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [tick]);

  useEffect(() => {
    const h = setInterval(() => {
      setNow(Date.now());
      setTick((n) => n + 1);
    }, 1000);
    return () => clearInterval(h);
  }, []);

  if (rows.length === 0) return null;

  return (
    <section className="outbox" aria-label={t('outbox-title')}>
      <h2 className="outbox-title">{t('outbox-title')}</h2>
      {rows.map((r) => (
        <Row key={r.id} row={r} now={now} onChange={() => setTick((n) => n + 1)} onDiscard={onDiscard} />
      ))}
    </section>
  );
}
