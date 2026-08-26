// Fails when the Rust pins in Cargo.toml can be lifted.
//
// Those pins hold the 2026-07-21 release set so that emnapi stays on a stable
// 1.x. Two independent upstream events would end that need, and this script
// exits non-zero on either, so a scheduled CI run is the notification:
//
//   1. emnapi ships a stable 2.x  -> take it, and unpin.
//   2. napi-build stops emitting a hard `--export` for the emnapi 2 symbols
//      -> emnapi 1.x links against current napi again, and unpin.
//
// The network calls live in main(); `--selftest` exercises the decision offline
// so the failure path is covered too -- a notification that cannot fire is
// worth nothing.

const NPM = 'https://registry.npmjs.org/emnapi/latest';
const WASI_RS =
  'https://raw.githubusercontent.com/napi-rs/napi-rs/main/crates/build/src/wasi.rs';

/** Why the pins look obsolete. Empty means keep them. */
export function assess(emnapiLatest, wasiRs) {
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
  return reasons;
}

async function get(url, as) {
  const r = await fetch(url, { headers: { 'user-agent': 'resvg-napi check:pins' } });
  if (!r.ok) throw new Error(`${url} -> HTTP ${r.status}`);
  return as === 'json' ? r.json() : r.text();
}

async function selftest() {
  const assert = (await import('node:assert/strict')).default;
  const HARD = 'println!("cargo:rustc-link-arg=--export=emnapi_create_env");';
  const LENIENT = 'println!("cargo:rustc-link-arg=--export-if-defined=emnapi_create_env");';

  assert.deepEqual(assess('1.11.3', HARD), [], 'today: both pins still needed');

  const stable2 = assess('2.0.0', HARD);
  assert.equal(stable2.length, 1);
  assert.match(stable2[0], /no longer 1\.x/);

  const fixed = assess('1.11.3', LENIENT);
  assert.equal(fixed.length, 1);
  assert.match(fixed[0], /--export-if-defined/);

  // a prerelease is not a stable 2.x, and must not fire
  assert.deepEqual(assess('1.12.0', HARD), [], 'a new 1.x is still 1.x');

  assert.equal(assess('2.1.0', LENIENT).length, 2, 'both conditions can hold at once');
  assert.match(assess('1.11.3', 'nothing here').at(0), /no longer exports/);

  console.log('ok — check-pins: 6 checks passed');
}

async function main() {
  const [{ version }, wasi] = await Promise.all([get(NPM, 'json'), get(WASI_RS)]);
  const reasons = assess(version, wasi);
  if (reasons.length) {
    console.error('The Cargo.toml pins look obsolete:\n');
    for (const r of reasons) console.error(`  - ${r}`);
    console.error('\nSee the comment above `napi` in Cargo.toml.');
    process.exit(1);
  }
  console.log(`pins still needed — emnapi latest ${version}, napi-build still hard-exports emnapi_create_env`);
}

await (process.argv.includes('--selftest') ? selftest() : main());
