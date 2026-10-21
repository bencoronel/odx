// Kept apart from ShellPage so App can flush on lock without loading the Terminal chunk.
let flushPending: (() => Promise<void>) | undefined;
export const registerShellFlush = (flush: (() => Promise<void>) | undefined) => { flushPending = flush; };
export const currentShellFlush = () => flushPending;
// shortcut: locks started outside the window (tray, sleep, auto-lock) can't flush and may lose up to 800ms of Shell changes; upgrade by having the backend request a flush before locking.
/** Save pending session changes now; the vault rejects saves once locked. */
export const flushShellSessions = () => flushPending?.() ?? Promise.resolve();
