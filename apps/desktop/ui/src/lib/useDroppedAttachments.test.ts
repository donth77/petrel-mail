import { beforeEach, describe, expect, it, vi } from 'vitest';
import type { Draft } from '../components/Compose';
import type { Attached } from './attachments';
import { FilesLeftOut, keptWhole } from './files-left-out';
import { useDroppedAttachments } from './useDroppedAttachments';

/* A dropped file's chip is drawn before its bytes are copied, and every save
 * waits for the copy. The saves run one after another on a chain, and each is
 * handed the message as it stood when it was asked for: an autosave and a Send
 * can both be holding a message that still shows the chip as copying.
 *
 * The hook runs outside React. It keeps its bookkeeping in refs, so standing
 * in for `useRef` is the whole of it, and the copies finish when a test says
 * so rather than when a disk does. */
vi.mock('react', async (importOriginal) => ({
  ...(await importOriginal<typeof import('react')>()),
  useRef: (init: unknown) => ({ current: init }),
}));

const blank: Draft = { to: 'dana@example.com', cc: '', subject: 'Plans', body: '', html: '' };

/** A file as a drop hands it over: the size at once, the bytes on request. */
function file(name: string, size = 4): File {
  return { name, size, arrayBuffer: async () => new ArrayBuffer(size) } as unknown as File;
}

/** Where the shell would say it put `name`. */
const staged = (name: string, size = 4): Attached => ({ path: `/staged/${name}`, name, size });

/** Lets a copy's own awaits run, so it has asked the shell to stage. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

/** The composer: the message on screen, copies that end only when told to,
 *  and what the person was told. */
let state: {
  draft: Draft | null;
  copies: { resolve: (a: Attached) => void; reject: (e: unknown) => void }[];
  failed: unknown[];
  rejected: string[];
};
let drops: ReturnType<typeof useDroppedAttachments>;

beforeEach(() => {
  state = { draft: blank, copies: [], failed: [], rejected: [] };
  drops = useDroppedAttachments({
    getDraft: () => state.draft,
    setDraft: (update) => {
      state.draft = typeof update === 'function' ? update(state.draft) : update;
    },
    stage: () =>
      new Promise<Attached>((resolve, reject) => {
        state.copies.push({ resolve, reject });
      }),
    onRejected: (names) => state.rejected.push(...names),
    onFailed: (error) => state.failed.push(error),
  });
});

/** What Send and Close do: the message leaves the composer before its save
 *  has run. */
const close = () => {
  state.draft = null;
};

describe('useDroppedAttachments', () => {
  it('a save queued behind another still carries the file being copied', async () => {
    // Send pressed while an autosave waits on a large file's copy. The first
    // save to finish used to forget the copy, so Send's save, holding the
    // same message, found nothing and went without the file.
    drops.drop([file('big.pdf')]);
    const shown = state.draft!;
    const autosave = drops.finish(shown);
    close();
    const send = autosave.then(() => drops.finish(shown));
    await settle();
    state.copies[0]!.resolve(staged('big.pdf'));
    expect((await autosave).draft.attachments).toEqual([staged('big.pdf')]);
    expect((await send).draft.attachments).toEqual([staged('big.pdf')]);
  });

  it('a message that comes back after a failed send keeps the file it was copying', async () => {
    // A send that cannot be queued puts the message back as it was handed
    // over, chip and all. The copy finished meanwhile, and the next save
    // must still find it.
    drops.drop([file('big.pdf')]);
    const sent = state.draft!;
    close();
    const send = drops.finish(sent);
    await settle();
    state.copies[0]!.resolve(staged('big.pdf'));
    await send;
    state.draft = sent;
    const saved = await drops.finish(sent);
    expect(saved.draft.attachments).toEqual([staged('big.pdf')]);
    expect(state.draft?.attachments).toEqual([staged('big.pdf')]);
    expect(state.draft?.copying ?? []).toEqual([]);
  });

  it('a file removed while another is copying stays removed', async () => {
    state.draft = { ...blank, attachments: [staged('a.pdf')] };
    drops.drop([file('big.pdf')]);
    const autosave = drops.finish(state.draft!);
    state.draft = { ...state.draft!, attachments: [] };
    await settle();
    state.copies[0]!.resolve(staged('big.pdf'));
    await autosave;
    expect(state.draft?.attachments).toEqual([staged('big.pdf')]);
  });

  it('a copy that fails after the composer has gone is still reported', async () => {
    // The person saw the chip and pressed Send. The message goes without the
    // file, which is all there is to send, and they are told.
    drops.drop([file('big.pdf')]);
    const sent = state.draft!;
    close();
    const send = drops.finish(sent);
    await settle();
    state.copies[0]!.reject(new Error('disk full'));
    expect((await send).draft.attachments ?? []).toEqual([]);
    expect(state.failed).toHaveLength(1);
  });

  it('a failed copy is reported once, however many saves were waiting on it', async () => {
    drops.drop([file('big.pdf')]);
    const shown = state.draft!;
    const autosave = drops.finish(shown);
    const send = autosave.then(() => drops.finish(shown));
    await settle();
    state.copies[0]!.reject(new Error('disk full'));
    expect((await send).draft.attachments ?? []).toEqual([]);
    expect(state.failed).toHaveLength(1);
    expect(state.draft?.copying ?? []).toEqual([]);
  });

  it('a chip taken off on purpose is not reported when its copy fails', async () => {
    drops.drop([file('big.pdf')]);
    const shown = state.draft!;
    const autosave = drops.finish(shown);
    drops.remove(shown.copying![0]!.id);
    await settle();
    state.copies[0]!.reject(new Error('disk full'));
    expect((await autosave).draft.attachments ?? []).toEqual([]);
    expect(state.failed).toEqual([]);
  });

  it('a chip taken off on purpose stays off when its copy lands', async () => {
    drops.drop([file('big.pdf')]);
    const shown = state.draft!;
    const autosave = drops.finish(shown);
    drops.remove(shown.copying![0]!.id);
    await settle();
    state.copies[0]!.resolve(staged('big.pdf'));
    expect((await autosave).draft.attachments ?? []).toEqual([]);
    expect(state.draft?.attachments ?? []).toEqual([]);
  });

  it('a copy that lands after the composer moved on stays out of the next message', async () => {
    drops.drop([file('big.pdf')]);
    close();
    state.draft = { ...blank, subject: 'Something else' };
    await settle();
    state.copies[0]!.resolve(staged('big.pdf'));
    await settle();
    expect(state.draft?.attachments ?? []).toEqual([]);
    expect(state.draft?.copying ?? []).toEqual([]);
  });
});

/* Send, Send later, Pop out and a popped-out window's Close take the message
 * out of the composer, and a window can go with the copy's toast still in
 * it. So the save says what it left out, and they stop on it. */
describe('what a save left out', () => {
  const MB = 1024 * 1024;

  it('names a chip whose copy failed', async () => {
    drops.drop([file('big.pdf')]);
    const save = drops.finish(state.draft!);
    await settle();
    state.copies[0]!.reject(new Error('disk full'));
    const { draft, leftOut } = await save;
    expect(draft.attachments ?? []).toEqual([]);
    expect(leftOut.map((c) => c.name)).toEqual(['big.pdf']);
    // Said by the copy, as the failure it was, and not again as a misfit.
    expect(state.failed).toHaveLength(1);
    expect(state.rejected).toEqual([]);
  });

  it('tells every save that was waiting on the failed copy', async () => {
    // Send queued behind an autosave: both were handed the chip, and Send is
    // the one that has to stop.
    drops.drop([file('big.pdf')]);
    const shown = state.draft!;
    const autosave = drops.finish(shown);
    close();
    const send = autosave.then(() => drops.finish(shown));
    await settle();
    state.copies[0]!.reject(new Error('disk full'));
    expect((await autosave).leftOut.map((c) => c.name)).toEqual(['big.pdf']);
    expect((await send).leftOut.map((c) => c.name)).toEqual(['big.pdf']);
    expect(state.failed).toHaveLength(1);
    expect(state.rejected).toEqual([]);
  });

  it('names a file refused once its bytes are counted', async () => {
    // The size on the drop was a guess; the bytes do not fit.
    const guessed = {
      name: 'huge.mov',
      size: 4,
      arrayBuffer: async () => new ArrayBuffer(26 * MB),
    } as unknown as File;
    drops.drop([guessed]);
    const { leftOut } = await drops.finish(state.draft!);
    expect(leftOut.map((c) => c.name)).toEqual(['huge.mov']);
    expect(state.rejected).toEqual(['huge.mov']);
  });

  it('names a copied file that no longer fits beside what the message carries', async () => {
    drops.drop([file('big.pdf', 10 * MB)]);
    // Picked while the copy ran. The picker counts what is attached, not
    // chips still copying, so the two together no longer fit.
    state.draft = { ...state.draft!, attachments: [staged('other.pdf', 15 * MB)] };
    const save = drops.finish(state.draft);
    await settle();
    state.copies[0]!.resolve(staged('big.pdf', 10 * MB));
    const { draft, leftOut } = await save;
    expect(draft.attachments).toEqual([staged('other.pdf', 15 * MB)]);
    expect(leftOut.map((c) => c.name)).toEqual(['big.pdf']);
    // Said as it landed on screen, and not again by the save.
    expect(state.rejected).toEqual(['big.pdf']);
  });

  it('says a copied file did not fit when it landed with no message on screen', async () => {
    // Send took the message away before the copy landed, so nothing on
    // screen could say the file did not fit. The save says it, once, however
    // many saves were holding the message.
    drops.drop([file('big.pdf', 10 * MB)]);
    state.draft = { ...state.draft!, attachments: [staged('other.pdf', 15 * MB)] };
    const sent = state.draft;
    close();
    const send = drops.finish(sent);
    const again = send.then(() => drops.finish(sent));
    await settle();
    state.copies[0]!.resolve(staged('big.pdf', 10 * MB));
    expect((await send).leftOut.map((c) => c.name)).toEqual(['big.pdf']);
    expect((await again).leftOut.map((c) => c.name)).toEqual(['big.pdf']);
    expect(state.rejected).toEqual(['big.pdf']);
  });

  it('does not name a chip taken off on purpose', async () => {
    drops.drop([file('big.pdf')]);
    const shown = state.draft!;
    const save = drops.finish(shown);
    drops.remove(shown.copying![0]!.id);
    await settle();
    state.copies[0]!.reject(new Error('disk full'));
    expect((await save).leftOut).toEqual([]);
  });

  it('does not name a copy that landed', async () => {
    drops.drop([file('big.pdf')]);
    const save = drops.finish(state.draft!);
    await settle();
    state.copies[0]!.resolve(staged('big.pdf'));
    const { draft, leftOut } = await save;
    expect(draft.attachments).toEqual([staged('big.pdf')]);
    expect(leftOut).toEqual([]);
  });

  it('a send waiting on a copy that fails writes the message and is not queued', async () => {
    // The chain both composers run for Send: finish, write, then queue.
    // It used to queue the message without the file.
    const written: Draft[] = [];
    const queued: number[] = [];
    drops.drop([file('big.pdf')]);
    const sent = state.draft!;
    close();
    const send = drops
      .finish(sent)
      .then((finished) => {
        written.push(finished.draft);
        return keptWhole(7, finished, true);
      })
      .then((id) => {
        queued.push(id);
      });
    await settle();
    state.copies[0]!.reject(new Error('disk full'));
    const stopped = await send.then(
      () => null,
      (e: unknown) => e,
    );
    expect(stopped).toBeInstanceOf(FilesLeftOut);
    expect(queued).toEqual([]);
    expect(written).toHaveLength(1);
    // What comes back to the composer: the words, in the row just written,
    // with no chip left spinning.
    expect((stopped as FilesLeftOut).draft).toMatchObject({
      subject: 'Plans',
      savedId: 7,
      copying: [],
      attachments: [],
    });
  });
});
