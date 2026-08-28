/**
 * Shows which characters earned a fuzzy match, so the ordering never looks
 * arbitrary.
 *
 * Shared rather than owned by the dialog picker: any list filtered by
 * `fuzzyMatch` owes the reader the same explanation of why these rows and why
 * in this order, and two implementations of that would eventually disagree
 * about which characters counted.
 */
export function Highlight({ text, hits }: { text: string; hits: number[] }) {
  if (hits.length === 0) return <>{text}</>;
  const set = new Set(hits);
  return (
    <>
      {[...text].map((ch, i) => (set.has(i) ? <span className="hit" key={i}>{ch}</span> : ch))}
    </>
  );
}
