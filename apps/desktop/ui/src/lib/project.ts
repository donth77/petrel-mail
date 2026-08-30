/* Where Petrel itself lives.
 *
 * Separate from lib/links.ts, which is about links found in *mail* and whether
 * they can be trusted. These are the project's own addresses, and they are
 * here rather than at each use because they now have two: the Help pane's
 * footer and the Help menu. A URL written twice is one that will eventually be
 * moved once, and a stale "report an issue" link fails exactly the person who
 * was trying to help.
 */

/** The repository. Everything else hangs off it. */
export const REPO_URL = 'https://github.com/donth77/petrel-mail';

/** Where a person reports something. */
export const ISSUES_URL = `${REPO_URL}/issues`;

/** The README, which is the closest thing to a front page. The fragment is the
 *  heading it opens with, so the link lands on the title rather than wherever
 *  GitHub decides to scroll. */
export const SOURCE_URL = `${REPO_URL}#petrel`;

/** Every release, newest first. Where somebody goes to read what changed in a
 *  version they do not have, or to fetch one the updater cannot reach. */
export const RELEASES_URL = `${REPO_URL}/releases`;

/** The newest release. GitHub resolves `latest` itself, so this cannot go
 *  stale the way a pinned tag would. */
export const LATEST_RELEASE_URL = `${REPO_URL}/releases/latest`;
