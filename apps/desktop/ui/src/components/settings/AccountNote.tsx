import { useEffect, useState } from 'react';
import { api, type Account } from '../../lib/api';
import { t } from '../../lib/strings';

/**
 * Which account a per-account pane is talking about.
 *
 * Rules and identities belong to one account, and the pane silently showed
 * whichever one was active — so with two accounts set up, the same screen
 * meant two different things and said nothing about which. Rules the user
 * could not find looked deleted rather than filed under the other address.
 *
 * Hidden on a single-account store, where naming the only account there is
 * would be noise: the same line the storage pane draws for its per-account
 * breakdown.
 */
export function AccountNote() {
  const [accounts, setAccounts] = useState<Account[]>([]);
  useEffect(() => {
    let live = true;
    api
      .accounts()
      .then((a) => live && setAccounts(a))
      .catch(() => {});
    return () => {
      live = false;
    };
  }, []);

  if (accounts.length < 2) return null;
  const active = accounts.find((a) => a.active);
  if (!active) return null;
  return <p className="pane-account">{t('settings-for-account', { email: active.email })}</p>;
}
