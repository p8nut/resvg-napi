// Watches for the npm names to become publishable again.
//
// 0.1.0 was unpublished by mistake. npm blocks republishing a name for 24 hours
// after that, but the block outlived the clock: more than a day later the
// registry still answered
//
//   409 Conflict -- Failed to save packument. A common cause is if you try to
//   publish a new package before the previous package has been fully processed.
//
// and twelve of the fourteen packuments had not been touched since. There is
// nothing to poll by hand every morning, so this does it.
//
// Red means go, the same convention as the `pins` job: a scheduled run that
// fails is the notification. Green means still blocked, which is the boring
// answer and deserves no attention.
//
// Read-only against the public registry, so it needs no token.
import { readFileSync, readdirSync, existsSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

/** Every name a release publishes: the root and its platform packages. */
export function names(rootManifest, platformManifests) {
  return [rootManifest.name, ...platformManifests.map((m) => m.name)].sort();
}

/**
 * What a packument says about one name.
 *
 * `unpublished` is the tombstone npm leaves behind, and it is what the 409
 * tracks -- a name carrying one cannot receive a new packument. A 404 means the
 * record is gone entirely, which is the cleanest form of free.
 */
export function state(status, packument) {
  if (status === 404) return 'free';
  if (status !== 200) return `unknown (HTTP ${status})`;
  if (Object.keys(packument?.versions ?? {}).length > 0) return 'published';
  if (packument?.time?.unpublished) return 'blocked';
  return 'free';
}

/** Whether a release can be attempted, and what to say about it. */
export function verdict(states) {
  const blocked = Object.entries(states)
    .filter(([, s]) => s === 'blocked')
    .map(([n]) => n);
  // `published` is not in the way: only the exact version already there is
  // spent, and a release moves the number anyway.
  return { ready: blocked.length === 0, blocked };
}

if (process.argv.includes('--selftest')) {
  const assert = (await import('node:assert/strict')).default;
  assert.deepEqual(names({ name: 'r' }, [{ name: 'r-b' }, { name: 'r-a' }]), ['r', 'r-a', 'r-b']);

  assert.equal(state(404, null), 'free');
  assert.equal(state(200, { versions: { '1.0.0': {} } }), 'published');
  assert.equal(state(200, { time: { unpublished: { time: 'x' } } }), 'blocked');
  assert.equal(state(200, { time: { modified: 'x' } }), 'free');
  // a tombstone with versions is a name that was republished: not blocked
  assert.equal(state(200, { versions: { '1.0.0': {} }, time: { unpublished: {} } }), 'published');
  assert.match(state(500, null), /unknown/);

  assert.deepEqual(verdict({ a: 'free', b: 'published' }), { ready: true, blocked: [] });
  const stuck = verdict({ a: 'free', b: 'blocked', c: 'blocked' });
  assert.equal(stuck.ready, false);
  assert.deepEqual(stuck.blocked, ['b', 'c']);
  // one blocked name is enough to hold the release
  assert.equal(verdict({ a: 'blocked' }).ready, false);
  assert.equal(verdict({}).ready, true);
  console.log('ok — check-npm-window: 11 checks passed');
  process.exit(0);
}

const root = JSON.parse(readFileSync('package.json', 'utf8'));
const platforms = readdirSync('npm')
  .filter((d) => existsSync(join('npm', d, 'package.json')))
  .map((d) => JSON.parse(readFileSync(join('npm', d, 'package.json'), 'utf8')));

const states = {};
for (const n of names(root, platforms)) {
  // Cache-busted: a stale CDN answer here would report a window that is not open.
  const r = await fetch(`https://registry.npmjs.org/${n}?t=${Date.now()}`, {
    headers: { 'user-agent': 'resvg-napi npm-window' },
  });
  states[n] = state(r.status, r.ok ? await r.json() : null);
}

const { ready, blocked } = verdict(states);
for (const [n, s] of Object.entries(states)) console.log(`${s.padEnd(10)} ${n}`);

const summary = process.env.GITHUB_STEP_SUMMARY;
const line = ready
  ? `**Publishable.** None of the ${Object.keys(states).length} names carries a tombstone. Tag \`v${root.version}\`.`
  : `Still blocked: ${blocked.length} of ${Object.keys(states).length}.`;
if (summary) {
  const { appendFileSync } = await import('node:fs');
  appendFileSync(summary, `${line}\n\n` +
    Object.entries(states).map(([n, s]) => `- \`${n}\` — ${s}`).join('\n') + '\n');
}

console.log(`\n${line}`);
if (ready) {
  console.log('::error::the npm names are free -- tag the release');
  process.exit(1); // red is the notification
}
