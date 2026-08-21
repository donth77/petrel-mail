# Bundled typefaces

All three ship under the SIL Open Font License 1.1, which permits redistribution
inside an application provided the licence travels with them.

| Family | Role | Upstream |
|---|---|---|
| Bricolage Grotesque | display | https://fonts.google.com/specimen/Bricolage+Grotesque |
| Public Sans | body | https://fonts.google.com/specimen/Public+Sans |
| Spline Sans Mono | metadata, times, counts | https://fonts.google.com/specimen/Spline+Sans+Mono |

Full licence text: https://openfontlicense.org/open-font-license-official-text/

Only the `latin` and `latin-ext` subsets are bundled. CJK rendering falls through
to the platform's own fonts, which is correct — these families do not cover CJK,
and shipping a CJK face would add megabytes to serve text the OS already renders.
