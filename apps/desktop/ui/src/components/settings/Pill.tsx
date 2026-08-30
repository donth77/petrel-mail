/** A segmented control: few options, all worth showing at once.
 *
 *  Lifted out of Appearance when the Sidebar pane needed one per mailbox row.
 *  Two copies of a control is two places for a focus ring to drift apart. */
export function Pill<T extends string>({
  value,
  options,
  onChange,
  label,
}: {
  value: T;
  options: { value: T; label: string }[];
  onChange: (v: T) => void;
  /** Names the group where the surrounding text does not, which is every row
   *  of a list where the same control appears over and over. */
  label?: string;
}) {
  return (
    <div className="pill" role="group" aria-label={label}>
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          className={o.value === value ? 'on' : undefined}
          aria-pressed={o.value === value}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  );
}
