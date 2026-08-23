/**
 * Pasting a picture into the composer.
 *
 * A pasted screenshot becomes part of the message body — embedded as a data:
 * URI while it is a draft, so it survives save and reload with no file
 * lifecycle at all, and split into a MIME part of its own at send time. The
 * decisions live here, out of the editor component, because what counts as an
 * embeddable paste is a policy, not a keystroke.
 */

/** The types worth embedding: what mail clients on the other end render.
 *
 *  SVG is deliberately absent — receiving sanitizers (ours included) treat it
 *  as markup, not pixels, and a picture that arrives blank is worse than one
 *  that was refused. TIFF and BMP for the same reason: browsers do not draw
 *  them, and most webmail is a browser. */
const EMBEDDABLE = new Set(['image/png', 'image/jpeg', 'image/gif', 'image/webp']);

/** The most a single pasted image may weigh, decoded.
 *
 *  Big enough for any screenshot — a 5K display's full screen is under half of
 *  it — and small enough that one paste cannot push the message past the
 *  common 25MB provider ceiling once base64 growth is counted. A photo larger
 *  than this belongs in an attachment, where the size is visible and the
 *  recipient saves it rather than scrolling past it. */
export const EMBED_CAP = 8 * 1024 * 1024;

/**
 * The images a paste carries, when the paste is *of* images.
 *
 * A clipboard with text on it — plain or formatted — is a text paste, even
 * when an image rides along: Word and Excel put a rendered picture of the
 * copied cells next to the real text, and pasting the picture instead of the
 * words would be surreal. Only a clipboard that is purely images (a
 * screenshot, a copied photo) is an image paste.
 */
export function pastedImages(data: DataTransfer | null): File[] {
  if (!data) return [];
  const flavours = Array.from(data.types);
  if (flavours.includes('text/plain') || flavours.includes('text/html')) return [];
  return Array.from(data.items)
    .filter((i) => i.kind === 'file' && EMBEDDABLE.has(i.type))
    .map((i) => i.getAsFile())
    .filter((f): f is File => f !== null);
}

/** Reads a file into the data: URI the editor embeds. */
export function asDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const r = new FileReader();
    r.onload = () => resolve(r.result as string);
    r.onerror = () => reject(r.error);
    r.readAsDataURL(file);
  });
}
