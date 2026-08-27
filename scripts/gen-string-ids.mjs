// Regenerates src/lib/string-ids.ts from locales/en.ftl.
//
//   node scripts/gen-string-ids.mjs
//
// Run it after adding or removing a string. A test fails if the two drift.
import { readFileSync, writeFileSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
// Repo-relative, so this runs from anywhere and on anyone's machine.
const here = dirname(fileURLToPath(import.meta.url));
const base = resolve(here, '../apps/desktop/ui/src');
const ftl = readFileSync(resolve(base, 'locales/en.ftl'), 'utf8');
const ids = [];
for (const line of ftl.split('\n')) {
  const m = /^([a-zA-Z][a-zA-Z0-9_-]*)\s*=/.exec(line);
  if (m) ids.push(m[1]);
}
const out = `/* Generated from locales/en.ftl by scripts/gen-string-ids.mjs. Do not edit.
 *
 * The ids live in the .ftl now, but call sites still want the compiler to
 * catch a typo. A test asserts this list and the .ftl agree, so the two
 * cannot drift apart quietly.
 */

export const STRING_IDS = [
${ids.map((i) => `  '${i}',`).join('\n')}
] as const;

export type StringId = (typeof STRING_IDS)[number];
`;
writeFileSync(resolve(base, 'lib/string-ids.ts'), out);
console.log('ids generated:', ids.length);
