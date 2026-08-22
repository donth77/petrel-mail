import type { Settings } from './settings';
import type { Thread } from './api';

/**
 * Whether an interruption is warranted, and what to say.
 *
 * Kept out of the component because it is a rules question, not a rendering
 * one: pause, level and what counts as priority all have to agree, and three
 * components each deciding separately is how a paused app still buzzes.
 */

/** Priority mail, for the "priority only" level.
 *
 *  Deliberately conservative for now — starred, or addressed to you rather
 *  than a list. A wrong "priority" that stays silent costs you a message; a
 *  wrong one that fires costs the setting its credibility, and people switch
 *  notifications off entirely rather than tune them. */
export function isPriority(t: Thread): boolean {
  return t.starred;
}

export function shouldNotify(settings: Settings, now: number): boolean {
  if (settings.notifyLevel === 'none') return false;
  const until = Number(settings.notifyPausedUntil) || 0;
  return now >= until;
}

/** The conversations from a batch that earn an interruption. */
export function notifiable(settings: Settings, arrivals: Thread[], now: number): Thread[] {
  if (!shouldNotify(settings, now)) return [];
  const unread = arrivals.filter((t) => t.unread);
  return settings.notifyLevel === 'priority' ? unread.filter(isPriority) : unread;
}

/**
 * Posts an OS notification, if the user allows it and the OS agrees.
 *
 * Every failure here is non-fatal by design: notification permission can be
 * refused, revoked, or unavailable entirely (an unsigned build on macOS is a
 * real case). The in-app toast has already been shown by the time this runs,
 * so a refusal costs the user nothing.
 */
export async function postDesktopNotification(title: string, body: string): Promise<boolean> {
  try {
    const mod = await import('@tauri-apps/plugin-notification');
    let granted = await mod.isPermissionGranted();
    if (!granted) {
      granted = (await mod.requestPermission()) === 'granted';
    }
    if (!granted) return false;
    mod.sendNotification({ title, body });
    return true;
  } catch {
    // Not running under Tauri, or the plugin is unavailable. Silence is the
    // right outcome; the toast already happened.
    return false;
  }
}
