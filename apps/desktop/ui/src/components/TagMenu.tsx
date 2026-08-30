import type React from 'react';
import { useState } from 'react';
import { Menu, MenuButton, MenuItem, MenuProvider, MenuSeparator } from '@ariakit/react';
import { MoreHorizontal, SquarePen, TagIcon } from 'lucide-react';
import { Icon } from './Icon';
import { t } from '../lib/strings';

/**
 * The colours a tag can be given.
 *
 * A fixed set rather than a colour wheel. A tag colour is read at a glance in a
 * dense list, so it has to stay distinguishable from the others and legible on
 * both grounds — a free picker mostly produces colours that fail one of those,
 * and nobody wants to choose a hex value to file an email.
 */
export const TAG_COLOURS = [
  '#c0392b',
  '#d35400',
  '#b7950b',
  '#2e7d5b',
  '#0e7c86',
  '#2b6cb0',
  '#6b46c1',
  '#a3427c',
] as const;

/**
 * Renaming, colouring and deleting one tag.
 *
 * On the rail beside the tag itself, because that is where the tag is when you
 * notice it is misspelled or the wrong colour. A tag is a name someone chose in
 * a hurry; being unable to correct one is what turns the list into a pile of
 * near-duplicates.
 */
export function TagMenu({
  name,
  colour,
  onRename,
  onColour,
  onDelete,
}: {
  name: string;
  colour: string;
  onRename: () => void;
  onColour: (colour: string) => void;
  onDelete: () => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <MenuProvider open={open} setOpen={setOpen} placement="bottom-end">
      <MenuButton
        className="tag-edit"
        aria-label={t('tag-edit')}
        // The rail row is itself a button, and a press here must not also
        // switch the view to the tag being edited.
        onClick={(e: React.MouseEvent) => e.stopPropagation()}
        onPointerDown={(e: React.PointerEvent) => e.stopPropagation()}
      >
        <Icon icon={MoreHorizontal} size={14} />
      </MenuButton>
      <Menu portal gutter={6} className="menu" aria-label={t('tag-edit', { name })}>
        <MenuItem className="menu-item" onClick={onRename}>
          <Icon icon={SquarePen} size={14} />
          <span className="menu-label">{t('tag-rename')}</span>
        </MenuItem>

        <div className="tag-section">{t('tag-colour')}</div>
        <div className="tag-swatches">
          {TAG_COLOURS.map((c) => (
            <button
              key={c}
              type="button"
              className="tag-swatch-pick"
              style={{ background: c }}
              aria-label={c}
              aria-pressed={colour.toLowerCase() === c}
              data-on={colour.toLowerCase() === c || undefined}
              onClick={() => {
                onColour(c);
                setOpen(false);
              }}
            />
          ))}
          <button
            type="button"
            className="tag-swatch-pick none"
            aria-label={t('tag-colour-none')}
            aria-pressed={!colour}
            data-on={!colour || undefined}
            onClick={() => {
              onColour('');
              setOpen(false);
            }}
          />
        </div>

        <MenuSeparator className="menu-sep" />
        <MenuItem className="menu-item danger" onClick={onDelete}>
          {/* A tag is taken off every conversation carrying it rather than
              binned, so a bin would be the wrong picture. */}
          <Icon icon={TagIcon} size={14} />
          <span className="menu-label">{t('tag-delete')}</span>
        </MenuItem>
      </Menu>
    </MenuProvider>
  );
}
