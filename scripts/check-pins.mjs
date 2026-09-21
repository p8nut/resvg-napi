// Fails when a deliberate dependency hold in this repo can be lifted.
//
// Two unrelated holds live here, because a hold nobody is told about is just a
// stale version. Each exits non-zero once its reason is gone, so the scheduled
// CI run is the notification.
//
// 1. The Rust pins in Cargo.toml, which keep the 2026-07-21 release set so that
//    emnapi stays on a stable 1.x. Three independent upstream events would end
//    that need:
//
//      a. emnapi ships a stable 2.x -> take it, and unpin.
//      b. napi-build stops emitting a hard `--export` for the emnapi 2 symbols
//         -> emnapi 1.x links against current napi again, and unpin.
//      c. emnapi backports those symbols into its 1.x archive -> the export is
//         satisfiable without leaving 1.x, and unpin.
//
//    (c) is not implied by (a) or (b): latest would still be 1.x and the export
//    would still be hard, so the first two probes stay quiet while the pins are
//    liftable. It is also the direct question -- does the archive define the
//    symbol -- where the others are proxies for it.
//
// 2. flate2, held at 1.1.9 in Cargo.lock. 1.1.10 requires miniz_oxide ^0.9
//    while png still requires ^0.8, so taking it compiles two DEFLATE
//    implementations into every artifact. When png moves, that stops being true.
//
// The network calls live in main(); `--selftest` exercises the decisions
// offline so the failure paths are covered too -- a notification that cannot
// fire is worth nothing.

import zlib from 'node:zlib';

const NPM = 'https://registry.npmjs.org/emnapi/latest';
const WASI_RS =
  'https://raw.githubusercontent.com/napi-rs/napi-rs/main/crates/build/src/wasi.rs';
const PNG = 'https://crates.io/api/v1/crates/png';

/** What png requires today, and the whole reason flate2 is held. */
const PNG_MINIZ_OXIDE_REQ = '^0.8';

/** Why the Cargo.toml pins look obsolete. Empty means keep them. */
export function assess(emnapiLatest, wasiRs, archiveDefinesCreateEnv) {
  const reasons = [];
  if (!emnapiLatest.startsWith('1.')) {
    reasons.push(`emnapi latest is ${emnapiLatest}, no longer 1.x -- move to it and drop the pins`);
  }
  // The lenient form is what lets emnapi 1.x link: napi-build already uses it
  // for other optional symbols a few lines below these two.
  if (!wasiRs.includes('--export=emnapi_create_env')) {
    reasons.push(
      wasiRs.includes('--export-if-defined=emnapi_create_env')
        ? 'napi-build main now uses --export-if-defined for emnapi_create_env -- unpin once released'
        : 'napi-build main no longer exports emnapi_create_env at all -- re-check the wasm link',
    );
  }
  if (archiveDefinesCreateEnv) {
    reasons.push(
      `emnapi ${emnapiLatest} now carries emnapi_create_env -- the hard export is satisfiable on 1.x, so unpin`,
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
 * Does the published emnapi package carry `emnapi_create_env` anywhere?
 *
 * ponytail: a byte search over the unpacked tarball, not a tar walk plus a
 * symbol-table read. 1.11.3 contains the name nowhere at all -- not in an
 * archive, not in the JS -- and 2.0.0-alpha.5 defines it in
 * lib/wasm32-wasip1-threads/libemnapi-napi-rs-mt.a, so a search separates them
 * exactly. If some future 1.x mentions the name without the archive defining
 * it, this reports a liftable pin that is not: a noisy failure, not a silent
 * one, and the fix is to walk the tar and read the archive's index.
 */
async function emnapiCarriesCreateEnv(tarballUrl) {
  const r = await fetch(tarballUrl, { headers: { 'user-agent': 'resvg-napi check:pins' } });
  if (!r.ok) throw new Error(`${tarballUrl} -> HTTP ${r.status}`);
  const unpacked = zlib.gunzipSync(Buffer.from(await r.arrayBuffer()));
  return unpacked.includes('emnapi_create_env');
}

async function selftest() {
  const assert = (await import('node:assert/strict')).default;
  const HARD = 'println!("cargo:rustc-link-arg=--export=emnapi_create_env");';
  const LENIENT = 'println!("cargo:rustc-link-arg=--export-if-defined=emnapi_create_env");';

  assert.deepEqual(assess('1.11.3', HARD, false), [], 'today: all three pins still needed');

  const stable2 = assess('2.0.0', HARD, false);
  assert.equal(stable2.length, 1);
  assert.match(stable2[0], /no longer 1\.x/);

  const fixed = assess('1.11.3', LENIENT, false);
  assert.equal(fixed.length, 1);
  assert.match(fixed[0], /--export-if-defined/);

  // the backport path: still 1.x, export still hard, yet liftable
  const backport = assess('1.12.0', HARD, true);
  assert.equal(backport.length, 1);
  assert.match(backport[0], /satisfiable on 1\.x/);

  // a prerelease is not a stable 2.x, and must not fire
  assert.deepEqual(assess('1.12.0', HARD, false), [], 'a new 1.x is still 1.x');

  assert.equal(assess('2.1.0', LENIENT, true).length, 3, 'all three conditions can hold at once');
  assert.match(assess('1.11.3', 'nothing here', false).at(0), /no longer exports/);

  assert.deepEqual(assessFlate2('0.18.1', '^0.8'), [], 'today: png still pins flate2 back');
  const moved = assessFlate2('0.19.0', '^0.9');
  assert.equal(moved.length, 1);
  assert.match(moved[0], /miniz_oxide \^0\.9/);

  console.log('ok — check-pins: 11 checks passed');
}

async function main() {
  const [emnapi, wasi, png] = await Promise.all([get(NPM, 'json'), get(WASI_RS), get(PNG, 'json')]);

  const pngVersion = png.crate.max_stable_version;
  const pngDeps = await get(`${PNG}/${pngVersion}/dependencies`, 'json');
  const minizOxide = pngDeps.dependencies.find(
    (d) => d.crate_id === 'miniz_oxide' && d.kind === 'normal',
  );
  if (!minizOxide) throw new Error(`png ${pngVersion} no longer depends on miniz_oxide at all`);

  const reasons = [
    ...assess(emnapi.version, wasi, await emnapiCarriesCreateEnv(emnapi.dist.tarball)),
    ...assessFlate2(pngVersion, minizOxide.req),
  ];
  if (reasons.length) {
    console.error('A dependency hold looks obsolete:\n');
    for (const r of reasons) console.error(`  - ${r}`);
    console.error('\nSee the comment at the top of this file, and the one above `napi` in Cargo.toml.');
    process.exit(1);
  }
  console.log(
    `holds still needed — emnapi latest ${emnapi.version} without emnapi_create_env, ` +
      `napi-build still hard-exports it, png ${pngVersion} still on miniz_oxide ${minizOxide.req}`,
  );
}

await (process.argv.includes('--selftest') ? selftest() : main());
