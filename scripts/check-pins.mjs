// Fails when a deliberate dependency hold in this repo can be lifted.
//
// Two unrelated holds live here, because a hold nobody is told about is just a
// stale version. Each exits non-zero once its reason is gone, so the scheduled
// CI run is the notification.
//
// 1. emnapi on a 2.0 prerelease, 2.0.0-beta.2, in both package.json files.
//    Current napi links the wasm target against the emnapi 2 ABI, so 1.x is
//    out. It ends when emnapi ships a stable 2.x: move both package.json
//    files to it, together with an @napi-rs/wasm-runtime that accepts it --
//    1.2.6 takes emnapi 2 as a peer at exactly 2.0.0-beta.2, so emnapi alone
//    is a peer conflict. Dependabot's `napi` group already moves them as one.
//
// 2. flate2, held at 1.1.9 in Cargo.lock. 1.1.10 requires miniz_oxide ^0.9
//    while png still requires ^0.8, so taking it compiles two DEFLATE
//    implementations into every artifact. When png moves, that stops being true.
//
// The network calls live in main(); `--selftest` exercises the decisions
// offline so the failure paths are covered too -- a notification that cannot
// fire is worth nothing.

const EMNAPI = 'https://registry.npmjs.org/emnapi/latest';
const PNG = 'https://crates.io/api/v1/crates/png';

/** What png requires today, and the whole reason flate2 is held. */
const PNG_MINIZ_OXIDE_REQ = '^0.8';

/** Why the emnapi prerelease hold looks obsolete. Empty means keep it. */
export function assess(emnapiLatest) {
  // A stable 2.x or later: no prerelease tag, major past 1.
  return /^([2-9]|\d{2,})\.\d+\.\d+$/.test(emnapiLatest)
    ? [`emnapi ${emnapiLatest} is out as latest -- move off the 2.0.0-beta.2 prerelease`]
    : [];
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

async function get(url) {
  const r = await fetch(url, { headers: { 'user-agent': 'resvg-napi check:pins' } });
  if (!r.ok) throw new Error(`${url} -> HTTP ${r.status}`);
  return r.json();
}

async function selftest() {
  const assert = (await import('node:assert/strict')).default;
  assert.deepEqual(assess('1.11.3'), [], 'today: the beta still needed');
  assert.deepEqual(assess('2.0.0-beta.2'), [], 'a prerelease as latest is not a stable');

  const stable = assess('2.0.0');
  assert.equal(stable.length, 1);
  assert.match(stable[0], /emnapi 2\.0\.0 is out/);

  assert.deepEqual(assessFlate2('0.18.1', '^0.8'), [], 'today: png still pins flate2 back');
  const moved = assessFlate2('0.19.0', '^0.9');
  assert.equal(moved.length, 1);
  assert.match(moved[0], /miniz_oxide \^0\.9/);

  console.log('ok — check-pins: 6 checks passed');
}

async function main() {
  const [emnapi, png] = await Promise.all([get(EMNAPI), get(PNG)]);

  const pngVersion = png.crate.max_stable_version;
  const pngDeps = await get(`${PNG}/${pngVersion}/dependencies`);
  const minizOxide = pngDeps.dependencies.find(
    (d) => d.crate_id === 'miniz_oxide' && d.kind === 'normal',
  );
  if (!minizOxide) throw new Error(`png ${pngVersion} no longer depends on miniz_oxide at all`);

  const reasons = [
    ...assess(emnapi.version),
    ...assessFlate2(pngVersion, minizOxide.req),
  ];
  if (reasons.length) {
    console.error('A dependency hold looks obsolete:\n');
    for (const r of reasons) console.error(`  - ${r}`);
    console.error('\nSee the comment at the top of this file.');
    process.exit(1);
  }
  console.log(
    `holds still needed — emnapi latest ${emnapi.version}, png ${pngVersion} still on miniz_oxide ${minizOxide.req}`,
  );
}

/** Crates `cargo update` must leave alone, for CI's `lock` job (`--held`). */
const HELD = ['flate2'];

if (process.argv.includes('--held')) console.log(HELD.join('|'));
else await (process.argv.includes('--selftest') ? selftest() : main());
