import type { Draft } from '../components/Compose';

/** How long typing has to pause before the draft is written. */
export const AUTOSAVE_MS = 1500;

/** Whether a draft holds anything worth keeping. Cc and attachments count:
 *  a draft that was only a Cc line was once dropped on close. */
export function draftHasContent(d: Draft): boolean {
  return Boolean(
    d.to || d.cc || d.subject || d.body.trim() || (d.attachments?.length ?? 0) > 0,
  );
}

/** What a save would write, as one string, so "changed since the last save"
 *  is a comparison rather than a guess. The saved id is left out on purpose:
 *  it changes when a save lands, and that must not count as an edit. */
export function draftSignature(d: Draft): string {
  return JSON.stringify([
    d.to,
    d.cc,
    d.subject,
    d.body,
    d.html,
    d.inReplyTo ?? null,
    d.references ?? [],
    (d.attachments ?? []).map((a) => a.path),
  ]);
}

/** The save state of one message in the composer: the row it lives in once
 *  a save has landed, and what that row holds. */
export type ComposerSlot = { id: number | null; signature: string | null };

/** The slot a message starts with. A draft from the store matches its stored
 *  copy. A fresh message matches what it starts with, signature and all, so
 *  a composer that was opened and left alone never writes a draft of nothing
 *  but the signature — as it would if the baseline were "nothing". */
export function slotFor(d: Draft): ComposerSlot {
  return { id: d.savedId ?? null, signature: draftSignature(d) };
}

/** Whether the message holds something its row does not: content that has
 *  changed since the last save, or since it was opened. */
export function unsaved(d: Draft, slot: ComposerSlot): boolean {
  return draftHasContent(d) && draftSignature(d) !== slot.signature;
}
