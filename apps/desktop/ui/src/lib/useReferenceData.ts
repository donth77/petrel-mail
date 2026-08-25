import { useEffect, useState } from 'react';
import { api, type Account, type Folder, type Identity, type Tag } from './api';

/**
 * The account's reference data — tags, folders, accounts, identity — and the
 * one effect that loads it.
 *
 * Tags come from the account, so one that has no conversation on this page
 * still appears in the rail. Everything that used to read `accounts[0]` — the
 * rail's label, the composer's From, the Gmail-only note in the folder
 * picker — reads `activeAccount`, so a switch changes all of them.
 */
export function useReferenceData(seeding: boolean | undefined, accountEpoch: number) {
  const [tags, setTags] = useState<Tag[]>([]);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [accounts, setAccounts] = useState<Account[]>([]);
  const [identity, setIdentity] = useState<Identity | null>(null);
  // The account the window is showing.
  const activeAccount = accounts.find((a) => a.active) ?? accounts[0];

  useEffect(() => {
    let live = true;
    // Reported, not swallowed. A tag list that failed to load and an account
    // with no tags produce exactly the same empty picker, so a silent catch
    // here turns a broken call into "you have no tags" — which is the one
    // reading of it that stops anyone looking for the real cause.
    api
      .tags()
      .then((t) => live && setTags(t))
      .catch((e) => api.log(`list_tags failed: ${e}`));
    api.folders().then((f) => live && setFolders(f)).catch((e) => api.log(`folders failed: ${e}`));
    api.identity().then((i) => live && setIdentity(i)).catch((e) => api.log(`identity failed: ${e}`));
    api.accounts().then((a) => live && setAccounts(a)).catch(() => {});
    return () => {
      live = false;
    };
    // Deliberately *not* keyed on the message count.
    //
    // These four are reference data — they change when the user changes them,
    // not when mail arrives. Keyed on the count they re-ran on every sync poll,
    // and each re-run's cleanup set `live = false` on the request already in
    // flight, so the answer was thrown away when it landed. During a sync the
    // count changes faster than the round trip, so the list was cancelled over
    // and over and simply stayed empty: no tags in the rail, none in the
    // picker, and nothing logged, because nothing had failed.
    //
    // The seeding flag is the honest trigger. It changes when a sync starts and
    // when it finishes — twice, not sixty times — which is exactly when folders
    // may have appeared and a re-read is worth doing.
  }, [seeding, accountEpoch]);

  return { tags, setTags, folders, setFolders, accounts, setAccounts, activeAccount, identity };
}
