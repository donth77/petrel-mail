import { Menu, MenuButton, MenuProvider } from '@ariakit/react';
import { MoreVertical } from 'lucide-react';
import { Icon } from './Icon';
import { t } from '../lib/strings';
import { Tip } from './Tip';
import { ThreadMenuItems, type ThreadMenuProps } from './ThreadMenuItems';

/**
 * The overflow menu behind the ⋮ button.
 *
 * It used to open the command palette. That was a shortcut — the palette does
 * contain these commands — but it reads as a bug: a button anchored to one
 * conversation opening a full-screen searchable launcher is not what ⋮ means
 * anywhere else, and it loses the anchoring that tells you *which* conversation
 * you are acting on. The palette is still there on its own shortcut, for when
 * you want to search rather than point.
 *
 * Every item here has a keyboard equivalent, shown alongside — the menu is for
 * discovering them, not a substitute for learning them.
 */
export function MoreMenu(props: ThreadMenuProps) {
  return (
    <MenuProvider placement="bottom-end">
      <Tip label={t('reader-more')}>
        <MenuButton className="act-icon" aria-label={t('reader-more')}>
          <Icon icon={MoreVertical} />
        </MenuButton>
      </Tip>
      <Menu portal gutter={6} className="menu" aria-label={t('reader-more')}>
        <ThreadMenuItems {...props} />
      </Menu>
    </MenuProvider>
  );
}
