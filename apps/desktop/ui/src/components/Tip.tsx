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
  children,
  placement = 'top',
  when = true,
}: {
  label: string;
  children: ReactElement;
  placement?: 'top' | 'right' | 'bottom' | 'left';
  when?: boolean;
}) {
  if (!when) return children;
  return (
    <TooltipProvider placement={placement} timeout={120}>
      <TooltipAnchor render={children} />
      <Tooltip portal gutter={6} className="tip">
        {label}
      </Tooltip>
    </TooltipProvider>
  );
}
