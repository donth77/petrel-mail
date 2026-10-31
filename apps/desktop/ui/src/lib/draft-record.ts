import type { Draft } from '../components/Compose';
import type { DraftRecord } from './api';

export function draftFromRecord(d: DraftRecord): Draft {
  // The envelope says which files, `files` how big each one is. Without the
  // size a reopened file showed as 0 B and counted for nothing against the
  // attachment limit.
  const files = new Map(d.files.map((f) => [f.path, f]));
  return {
    to: d.to,
    cc: d.cc,
    // Saved before there was a Bcc, an envelope has no such field.
    bcc: d.envelope.bcc ?? '',
    subject: d.subject,
    body: d.body,
    html: d.html,
    savedId: d.id,
    inReplyTo: d.envelope.in_reply_to,
    references: d.envelope.references,
    attachments: d.envelope.attachments.map((path) => ({
      path,
      name: files.get(path)?.name ?? (path.split(/[\\/]/).pop() || path),
      size: files.get(path)?.size ?? 0,
    })),
  };
}
