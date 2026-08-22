import type { ReactElement } from 'react';
import { Tooltip, TooltipAnchor, TooltipProvider } from '@ariakit/react';

/**
 * Wraps a rail item in a tooltip while the rail is collapsed.
 *
 * The `title` attribute is not a tooltip in any sense that helps here: it waits
 * about a second before appearing, cannot be styled to match anything, and on a
 * strip of unlabelled icons that delay is the whole cost — you hover to find
 * out what something is, and it does not tell you.
 *
 * Expanded, it renders the child untouched. A tooltip repeating a label the
 * user can already read is noise, and it would cover the item below it.
 */
export function RailTip({
  label,
  collapsed,
  children,
}: {
  label: string;
  collapsed: boolean;
  children: ReactElement;
}) {
  if (!collapsed) return children;
  return (
    <TooltipProvider placement="right" timeout={120}>
      <TooltipAnchor render={children} />
      <Tooltip portal gutter={6} className="tip">
        {label}
      </Tooltip>
    </TooltipProvider>
  );
}
