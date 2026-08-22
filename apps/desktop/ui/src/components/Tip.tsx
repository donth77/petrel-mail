import type { ReactElement } from 'react';
import { Tooltip, TooltipAnchor, TooltipProvider } from '@ariakit/react';

/**
 * One tooltip for the whole app.
 *
 * The `title` attribute is not a substitute and mixing the two is worse than
 * either alone: they look nothing alike, appear on different delays, and sit in
 * different places, so the same gesture produces two different-looking answers
 * depending on which control you happen to be pointing at.
 *
 * `when` exists because a tooltip is sometimes redundant — a rail item whose
 * label is already on screen does not need one, and showing it would cover the
 * item below.
 */
export function Tip({
  label,
  keys,
  children,
  placement = 'top',
  when = true,
}: {
  label: string;
  /** Shortcut keys, drawn as caps rather than written into the label.
   *
   *  "Archive (E)" reads as prose and the key disappears into it; a cap reads
   *  as a key. They are the same caps the shortcuts dialog uses, so the thing
   *  you learn from a tooltip is the thing you recognise in the reference —
   *  two different renderings of one keystroke is how people end up believing
   *  there are two. */
  keys?: string[];
  children: ReactElement;
  placement?: 'top' | 'right' | 'bottom' | 'left';
  when?: boolean;
}) {
  if (!when) return children;
  return (
    <TooltipProvider placement={placement} timeout={120}>
      <TooltipAnchor render={children} />
      <Tooltip portal gutter={6} className="tip">
        <span>{label}</span>
        {keys?.map((k) => (
          <span className="kbd" key={k}>
            {k}
          </span>
        ))}
      </Tooltip>
    </TooltipProvider>
  );
}
