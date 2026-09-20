/** How the app looks, as the frame's URL carries it.
 *
 *  The message frame is a document on another origin: it cannot see the app's
 *  stylesheet, so everything about the app's appearance has to arrive in its
 *  URL and be baked into the document that comes back. Pushing it in after
 *  first paint is a flash of the wrong colour on every message open.
 *
 *  That makes this string the dependency. The effect that builds the frame URL
 *  watches what this returns rather than a hand-kept list of what goes into
 *  it, so something new to bake in cannot be added without an open message
 *  picking it up. It has been forgotten twice now: the theme left every open
 *  message on the old palette until it was closed and reopened, and once the
 *  dark ground started following the accent, the accent did the same. */
export function frameLook(look: {
  /** Already resolved to light or dark. The frame's transform has to decide
   *  now, and "system" is only answerable on the app's side of the wall. */
  dark: boolean;
  /** This one message, rendered light whatever the app is wearing. */
  forceLight: boolean;
  /** The Appearance accent, with or without its `#`. */
  accent: string;
}): string {
  const parts = [`theme=${look.dark ? 'dark' : 'light'}`];
  if (look.forceLight) parts.push('force=light');
  // The `#` would start a fragment, so it comes off here and the frame puts it
  // back, after checking that what arrived is six hex digits.
  parts.push(`accent=${look.accent.replace('#', '')}`);
  return parts.join('&');
}

/** The frame URL: the single-use body URL, then how the app looks.
 *
 *  The body URL may already carry a query of its own, which is why this is a
 *  function rather than a template at the call site. */
export function frameUrl(body: string, look: string): string {
  return `${body}${body.includes('?') ? '&' : '?'}${look}`;
}
