// Kept apart from ShellPage so App can flush on lock without loading the Terminal chunk.
let flushPending: (() => Promise<void>) | undefined;
export const registerShellFlush = (flush: (() => Promise<void>) | undefined) => { flushPending = flush; };
export const currentShellFlush = () => flushPending;
/** Save pending session changes now; the vault rejects saves once locked. */
export const flushShellSessions = () => flushPending?.() ?? Promise.resolve();
