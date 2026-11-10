// The mod's own log, next to berbr's (`berbr logs` shows both): what the mod ran
// and how it went, including what fails before berbr even starts. Commands that
// change something are always written; snapshots only in debug mode, which
// follows berbr's (`debug = true` in its config, or BERBR_DEBUG=1).
// Mods may write only under the app home, and fs has no append: the file is
// rewritten, one write at a time, and cut to its newer half past the limit.
import { readFile, writeFile, mkdir } from "fs/promises";
import { appHome, shared, setShared } from "herdr";

const LIMIT = 512 * 1024;
let queue = Promise.resolve();
let debug = false;

// Shared too: forms run in their own views and never load a snapshot.
export function setDebug(on) {
  debug = Boolean(on);
  if (Boolean(shared("berbr.debug")) !== debug) setShared("berbr.debug", debug);
}
export function debugEnabled() { return debug || Boolean(shared("berbr.debug")); }
export function logDir() { return `${appHome()}/logs`; }
export function logPath() { return `${logDir()}/berbr-mod.log`; }

/** Appends a timestamped line; resolves once written. A failed write is dropped. */
export function log(text) {
  const line = `${new Date().toISOString()} ${String(text).replace(/\n/g, "\n    ")}\n`;
  queue = queue.then(async () => {
    await mkdir(logDir(), {recursive:true});
    let current = "";
    try { current = await readFile(logPath(), "utf8"); } catch (_) { /* first line */ }
    let next = current + line;
    if (next.length > LIMIT) next = next.slice(next.indexOf("\n", next.length - LIMIT / 2) + 1);
    await writeFile(logPath(), next);
  }).catch(() => {});
  return queue;
}

/** One berbr run: always unless it is a read, then only in debug mode. */
export function logRun(args, {cwd, ms, code = null, error = null, retried = null} = {}) {
  const read = args[0] === "app-snapshot" || args.includes("--dry-run");
  if (read && !debugEnabled() && !error && (code === 0 || code === null) && !retried) return queue;
  const where = cwd ? ` in ${cwd}` : "";
  const retry = retried ? ` (its folder is gone; ran from ${retried.to ?? "no folder"})` : "";
  const outcome = error ? `could not run: ${error}` : code === 0 ? "ok" : `exited ${code}`;
  return log(`berbr ${JSON.stringify(args)}${where}${retry} ${outcome} ${ms}ms`);
}
