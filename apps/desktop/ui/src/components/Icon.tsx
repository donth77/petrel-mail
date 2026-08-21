/* One place that fixes icon geometry to the design canvas: 1.7px stroke on a
   24px grid, round joins. Lucide's default is 2px, which reads heavier than the
   mockups. Icons beside a text label are decorative and hidden from screen
   readers; a standalone icon must be given a label by its parent control. */

import type { LucideIcon } from 'lucide-react';

type Props = { icon: LucideIcon; size?: number; className?: string };

export function Icon({ icon: Glyph, size = 15, className }: Props) {
  return (
    <Glyph size={size} strokeWidth={1.7} className={className} aria-hidden="true" focusable="false" />
  );
}
