/** Local media for <video>/<audio>/thumbnail elements goes through the
 * couple-media:// scheme (main-process protocol handler with Range support).
 * Plain file:// is blocked by Chromium when the page is served from the dev
 * server's http origin, so this is the ONLY sanctioned way to reference a
 * media file from the DOM. */
export function mediaUrl(path: string): string {
  // `standard` schemes require a host; "local" is a dummy the handler ignores.
  return `couple-media://local/${encodeURIComponent(path)}`;
}
