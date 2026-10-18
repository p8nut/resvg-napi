// Fails when a deliberate dependency hold in this repo can be lifted.
//
// Two unrelated holds live here, because a hold nobody is told about is just a
// stale version. Each exits non-zero once its reason is gone, so the scheduled
// CI run is the notification.
//
// 1. emnapi on a 2.0 prerelease, 2.0.0-alpha.6, in both package.json files.
//    Current napi links the wasm target against the emnapi 2 ABI, so 1.x is
//    out; and 2.0.0-beta.1 renamed the loader API (`napiModule` became
//    `addonModule`), which @napi-rs/wasm-runtime 1.2.5 still reads under the
//    old name -- its plugins throw on load. alpha.6 is the last release before
//    the rename. Two upstream events end this:
//
//      a. emnapi ships a stable 2.x -> move to it (once (b) holds too).
//      b. @napi-rs/wasm-runtime reads `addonModule` -> the betas, and later
//         the stable, become usable.
//
// 2. flate2, held at 1.1.9 in Cargo.lock. 1.1.10 requires miniz_oxide ^0.9
//    while png still requires ^0.8, so taking it compiles two DEFLATE
//    implementations into every artifact. When png moves, that stops being true.
//
// The network calls live in main(); `--selftest` exercises the decisions
// offline so the failure paths are covered too -- a notification that cannot
// fire is worth nothing.

import zlib from 'node:zlib';

const EMNAPI = 'https://registry.npmjs.org/emnapi/latest';
const RUNTIME = 'https://registry.npmjs.org/@napi-rs/wasm-runtime/latest';
const PNG = 'https://crates.io/api/v1/crates/png';

/** What png requires today, and the whole reason flate2 is held. */
const PNG_MINIZ_OXIDE_REQ = '^0.8';

/** Why the emnapi prerelease hold looks obsolete. Empty means keep it. */
export function assess(emnapiLatest, runtimeVersion, runtimeReadsAddonModule) {
  const reasons = [];
  // A stable 2.x or later: no prerelease tag, major past 1.
  if (/^([2-9]|\d{2,})\.\d+\.\d+$/.test(emnapiLatest)) {
    reasons.push(`emnapi ${emnapiLatest} is out as latest -- leave the 2.0.0-alpha.6 prerelease for it`);
  }
  if (runtimeReadsAddonModule) {
    reasons.push(
      `@napi-rs/wasm-runtime ${runtimeVersion} reads addonModule -- emnapi past beta.1 loads now, move off alpha.6`,
    );
  }
  return reasons;
}

/** Why the flate2 hold looks obsolete. Empty means keep it. */
export function assessFlate2(pngVersion, minizOxideReq) {
  // ponytail: exact-match the requirement rather than parse semver. The hold
  // exists for one state -- png on ^0.8 -- so any move off it is worth a human
  // look, including a cosmetic rewrite that changes nothing. Compare ranges
  // properly if png starts churning this field.
  return minizOxideReq === PNG_MINIZ_OXIDE_REQ
    ? []
    : [
        `png ${pngVersion} now requires miniz_oxide ${minizOxideReq}, not ${PNG_MINIZ_OXIDE_REQ} -- ` +
          'flate2 1.1.10+ may stop duplicating DEFLATE, so re-check the hold in Cargo.lock',
      ];
}

async function get(url, as) {
  const r = await fetch(url, { headers: { 'user-agent': 'resvg-napi check:pins' } });
  if (!r.ok) throw new Error(`${url} -> HTTP ${r.status}`);
  return as === 'json' ? r.json() : r.text();
}

/**
 * Does the published package's code mention `name` anywhere?
 *
 * ponytail: a byte search over the unpacked tarball, not a parse. 1.2.5
 * contains `addonModule` nowhere; a runtime that adopted the rename names it
 * in its plugins. A mention without real support would report a liftable hold
 * that is not: a noisy failure, not a silent one.
 */
async function tarballMentions(tarballUrl, name) {
  const r = await fetch(tarballUrl, { headers: { 'user-agent': 'resvg-napi check:pins' } });
  if (!r.ok) throw new Error(`${tarballUrl} -> HTTP ${r.status}`);
  return zlib.gunzipSync(Buffer.from(await r.arrayBuffer())).includes(name);
}

async function selftest() {
  const assert = (await import('node:assert/strict')).default;
  assert.deepEqual(assess('1.11.3', '1.2.5', false), [], 'today: alpha.6 still needed');
  assert.deepEqual(assess('2.0.0-beta.2', '1.2.5', false), [], 'a prerelease as latest is not a stable');

  const stable = assess('2.0.0', '1.2.5', false);
  assert.equal(stable.length, 1);
  assert.match(stable[0], /emnapi 2\.0\.0 is out/);

  const runtime = assess('1.11.3', '1.3.0', true);
  assert.equal(runtime.length, 1);
  assert.match(runtime[0], /wasm-runtime 1\.3\.0 reads addonModule/);

  assert.equal(assess('2.1.0', '1.3.0', true).length, 2, 'both can hold at once');

  assert.deepEqual(assessFlate2('0.18.1', '^0.8'), [], 'today: png still pins flate2 back');
  const moved = assessFlate2('0.19.0', '^0.9');
  assert.equal(moved.length, 1);
  assert.match(moved[0], /miniz_oxide \^0\.9/);

  console.log('ok — check-pins: 9 checks passed');
}

async function main() {
  const [emnapi, runtime, png] = await Promise.all([get(EMNAPI, 'json'), get(RUNTIME, 'json'), get(PNG, 'json')]);

  const pngVersion = png.crate.max_stable_version;
  const pngDeps = await get(`${PNG}/${pngVersion}/dependencies`, 'json');
  const minizOxide = pngDeps.dependencies.find(
    (d) => d.crate_id === 'miniz_oxide' && d.kind === 'normal',
  );
  if (!minizOxide) throw new Error(`png ${pngVersion} no longer depends on miniz_oxide at all`);

  const reasons = [
    ...assess(emnapi.version, runtime.version, await tarballMentions(runtime.dist.tarball, 'addonModule')),
    ...assessFlate2(pngVersion, minizOxide.req),
  ];
  if (reasons.length) {
    console.error('A dependency hold looks obsolete:\n');
    for (const r of reasons) console.error(`  - ${r}`);
    console.error('\nSee the comment at the top of this file.');
    process.exit(1);
  }
  console.log(
    `holds still needed — emnapi latest ${emnapi.version}, @napi-rs/wasm-runtime ${runtime.version} ` +
      `without addonModule, png ${pngVersion} still on miniz_oxide ${minizOxide.req}`,
  );
}

/** Crates `cargo update` must leave alone, for CI's `lock` job (`--held`). */
const HELD = ['flate2'];

if (process.argv.includes('--held')) console.log(HELD.join('|'));
else await (process.argv.includes('--selftest') ? selftest() : main());
