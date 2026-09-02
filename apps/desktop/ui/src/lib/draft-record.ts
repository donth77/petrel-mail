import type { Draft } from '../components/Compose';
import type { DraftRecord } from './api';

export function draftFromRecord(d: DraftRecord): Draft {
  return {
    to: d.to,
    cc: d.cc,
    subject: d.subject,
    body: d.body,
    html: d.html,
    savedId: d.id,
    inReplyTo: d.envelope.in_reply_to,
    references: d.envelope.references,
    attachments: d.envelope.attachments.map((path) => ({
      path,
      name: path.split(/[\\/]/).pop() || path,
      size: 0,
    })),
  };
}
