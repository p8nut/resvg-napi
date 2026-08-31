// resvg's own test corpus, rendered through this binding.
//
// 1739 SVGs with reference PNGs, and the 20 fonts they need -- upstream ships
// all of it, which is what makes the comparison deterministic. The crate does
// not: `exclude = ["tests"]`, so it is fetched from the tag matching our resvg
// version, or the references mean nothing.
//
// Settings replicate crates/resvg/tests/integration/main.rs exactly: width 300
// keeping the ratio, `resources_dir` set to the SVG's own directory, the five
// generic families pointed at the corpus fonts, and an un-premultiplied RGBA8
// comparison -- their harness demultiplies before diffing and `renderRaw`
// already hands that over.
//
//   npm run conformance             compare against the baseline
//   npm run conformance -- -w       rewrite the baseline
//   npm run conformance -- -s       selftest, offline

import { readFileSync, writeFileSync, readdirSync, existsSync } from 'node:fs';
import { join, dirname } from 'node:path';
import { createRequire } from 'node:module';

const ROOT = 'conformance/resvg/crates/resvg/tests';
const TESTLIST = join(ROOT, 'integration', 'render.rs');
const CORPUS = join(ROOT, 'tests');
const FONTS = join(ROOT, 'fonts');
const BASELINE = 'conformance/baseline.txt';
const WIDTH = 300; // IMAGE_SIZE in the upstream harness

/**
 * The cases upstream actually asserts on, read off its generated test list.
 *
 * The corpus holds more `.svg` files than that -- fixtures with a reference PNG
 * that no test names. Four of them looked like conformance failures until I
 * checked: three throw on an invalid document, and `no-size.svg` has a 500x500
 * reference for a 199x199 document, so it was never rendered by `render()` at
 * all. Scoping to the list is what makes "we agree with upstream" mean
 * something.
 */
export function testList(source) {
  return [...source.matchAll(/render\("tests\/([^"]+)"\)/g)]
    .map((m) => `${m[1]}.svg`)
    .sort();
}

/// A channel may differ by this much and still count as the same pixel.
///
/// Measured, not guessed: every sampled mismatch differs by exactly 1 on every
/// differing channel, and none differ in size. That is the alpha
/// demultiplication rounding -- upstream's harness demultiplies in Rust and its
/// references then go through oxipng -- not a rendering difference. At 0 the
/// metric reported 711 "failures" that were all this, and hid whatever else is
/// there.
const TOLERANCE = 1;

/** Pixels differing by more than `TOLERANCE` on any channel. */
export function diffPixels(a, b, tolerance = TOLERANCE) {
  let diff = 0;
  for (let i = 0; i < a.length; i += 4) {
    if (
      Math.abs(a[i] - b[i]) > tolerance ||
      Math.abs(a[i + 1] - b[i + 1]) > tolerance ||
      Math.abs(a[i + 2] - b[i + 2]) > tolerance ||
      Math.abs(a[i + 3] - b[i + 3]) > tolerance
    ) {
      diff++;
    }
  }
  return diff;
}

/**
 * What changed against the baseline. A file that got worse is a regression; one
 * that got better is news worth acting on, not a failure.
 *
 * @param {Map<string, number>} want  baseline: path -> differing pixels
 * @param {Map<string, number>} got
 */
/** A case that threw instead of rendering. Not a pixel count, and not comparable to one. */
export const THREW = -1;

export function compare(want, got) {
  const worse = [];
  const better = [];
  const gone = [];
  for (const [path, diff] of got) {
    const was = want.get(path);
    // Throwing is its own state, ranked above every pixel count rather than
    // sorted among them. It used to be stored as -1 and compared numerically, so
    // a case that started throwing went from 0 to -1 and was reported as an
    // *improvement* -- the one regression the suite exists to catch, counted as
    // progress.
    if (diff === THREW || was === THREW) {
      if (diff === THREW && was !== THREW) {
        worse.push({ path, was: was === undefined ? 'new' : was, now: 'threw' });
      } else if (was === THREW && diff !== THREW) {
        better.push({ path, was: 'threw', now: diff });
      }
      continue;
    }
    if (was === undefined) {
      if (diff > 0) worse.push({ path, was: 'new', now: diff });
    } else if (diff > was) worse.push({ path, was, now: diff });
    else if (diff < was) better.push({ path, was, now: diff });
  }
  for (const path of want.keys()) if (!got.has(path)) gone.push(path);
  return { worse, better, gone };
}

function parseBaseline(text) {
  const m = new Map();
  for (const line of text.split('\n')) {
    const t = line.trim();
    if (!t || t.startsWith('#')) continue;
    const i = t.lastIndexOf(' ');
    m.set(t.slice(0, i), Number(t.slice(i + 1)));
  }
  return m;
}

function formatBaseline(got) {
  const failing = [...got].filter(([, d]) => d > 0).sort(([a], [b]) => (a < b ? -1 : 1));
  const head = [
    '# resvg conformance: files whose render differs from upstream\'s reference,',
    '# and by how many pixels. Rendered at width 300 with the corpus fonts, the',
    '# same settings as crates/resvg/tests/integration/main.rs.',
    '#',
    `# ${failing.length} of ${got.size} files differ. Regenerate: npm run conformance -- -w`,
    '',
  ];
  return head.concat(failing.map(([p, d]) => `${p} ${d}`)).join('\n') + '\n';
}

function run() {
  const require = createRequire(import.meta.url);
  const { Resvg, FontDatabase } = require('../index.js');
  const { PNG } = require('pngjs');

  const fonts = new FontDatabase();
  for (const f of readdirSync(FONTS)) {
    if (/\.(ttf|otf|ttc)$/i.test(f)) fonts.loadFontFile(join(FONTS, f));
  }
  // The names the upstream harness sets, verbatim.
  fonts.setSerifFamily('Noto Serif');
  fonts.setSansSerifFamily('Noto Sans');
  fonts.setCursiveFamily('Yellowtail');
  fonts.setFantasyFamily('Sedgwick Ave Display');
  fonts.setMonospaceFamily('Noto Mono');

  const files = testList(readFileSync(TESTLIST, 'utf8'));
  // A corpus that moved, a regex that stopped matching, or a fetch that landed
  // the wrong tag all produce an empty list -- and an empty run passes, reports
  // "0 of 0 files differ", and rewrites the baseline to nothing. The bound is
  // loose on purpose: it catches "the list broke", not "upstream added a test".
  if (files.length < 1000) {
    throw new Error(
      `only ${files.length} cases in ${TESTLIST} -- the corpus or the test list is wrong, ` +
        'and an empty run would pass',
    );
  }
  const got = new Map();
  const errors = new Map();

  for (const rel of files) {
    const svgPath = join(CORPUS, rel);
    const pngPath = svgPath.replace(/\.svg$/, '.png');
    if (!existsSync(pngPath)) continue; // no reference, not a test
    try {
      const doc = new Resvg(readFileSync(svgPath), { resourcesDir: dirname(svgPath) }, fonts);
      const raw = doc.renderRaw({ width: WIDTH });
      const ref = PNG.sync.read(readFileSync(pngPath));
      if (ref.width !== raw.width || ref.height !== raw.height) {
        got.set(rel, ref.width * ref.height); // size mismatch: count it all
        continue;
      }
      got.set(rel, diffPixels(raw.data, ref.data));
    } catch (e) {
      errors.set(rel, e.message.split('\n')[0]);
      got.set(rel, THREW);
    }
  }
  return { got, errors };
}

async function selftest() {
  const assert = (await import('node:assert/strict')).default;

  // testList(): the paths upstream asserts on, sorted, `.svg` appended
  assert.deepEqual(
    testList('#[test] fn b() { assert_eq!(render("tests/z/b"), 0); }\n' +
             '#[test] fn a() { assert_eq!(render("tests/a"), 0); }\n' +
             'render_extra("tests/skipped")'),
    ['a.svg', 'z/b.svg'],
    'plain render() only, and sorted',
  );

  const B = (o) => new Map(Object.entries(o));
  assert.deepEqual(compare(B({ 'a': 5 }), B({ 'a': 5 })), { worse: [], better: [], gone: [] });

  const w = compare(B({ 'a': 5 }), B({ 'a': 9 }));
  assert.deepEqual(w.worse, [{ path: 'a', was: 5, now: 9 }]);

  // a case that starts throwing is a regression, whatever it did before
  assert.deepEqual(
    compare(B({ 'a': 0 }), B({ 'a': THREW })).worse,
    [{ path: 'a', was: 0, now: 'threw' }],
    'matching, then throwing -- the case this used to call an improvement',
  );
  assert.deepEqual(compare(B({ 'a': 0 }), B({ 'a': THREW })).better, []);
  assert.deepEqual(
    compare(B({}), B({ 'a': THREW })).worse,
    [{ path: 'a', was: 'new', now: 'threw' }],
    'and a new one that throws is not silently accepted',
  );
  // one that stops throwing is progress
  assert.deepEqual(
    compare(B({ 'a': THREW }), B({ 'a': 0 })).better,
    [{ path: 'a', was: 'threw', now: 0 }],
  );
  // still throwing is neither
  const same = compare(B({ 'a': THREW }), B({ 'a': THREW }));
  assert.deepEqual([same.worse, same.better], [[], []]);

  const b = compare(B({ 'a': 5 }), B({ 'a': 0 }));
  assert.deepEqual(b.better, [{ path: 'a', was: 5, now: 0 }]);

  // a file absent from the baseline is only news if it differs
  assert.deepEqual(compare(B({}), B({ 'n': 0 })).worse, []);
  assert.deepEqual(compare(B({}), B({ 'n': 3 })).worse, [{ path: 'n', was: 'new', now: 3 }]);

  // a file the corpus no longer has
  assert.deepEqual(compare(B({ 'x': 1 }), B({})).gone, ['x']);

  // the baseline round-trips, and passing files are not written
  const text = formatBaseline(B({ 'p': 0, 'f': 7 }));
  assert.deepEqual([...parseBaseline(text)], [['f', 7]]);

  // the tolerance: 1 is the same pixel, 2 is not
  const px = (v) => Uint8Array.from(v);
  assert.equal(diffPixels(px([10, 10, 10, 255]), px([11, 9, 10, 255])), 0, '±1 is the same pixel');
  assert.equal(diffPixels(px([10, 10, 10, 255]), px([12, 10, 10, 255])), 1, '±2 is not');
  assert.equal(diffPixels(px([0, 0, 0, 0]), px([0, 0, 0, 0])), 0);
  assert.equal(diffPixels(px([10, 10, 10, 255]), px([10, 10, 10, 250]), 8), 0, 'alpha counts too');

  console.log('ok — conformance: 17 checks passed');
}

async function main() {
  if (!existsSync(CORPUS)) {
    console.error(`corpus missing at ${CORPUS}`);
    console.error('fetch it with: npm run conformance:fetch');
    process.exit(1);
  }
  const write = process.argv.includes('-w') || process.argv.includes('--write');
  const { got, errors } = run();
  const failing = [...got.values()].filter((d) => d !== 0).length;
  console.log(
    `${got.size} files, ${got.size - failing} matching upstream's reference ` +
      `(within ${TOLERANCE}/255 per channel)`
  );
  if (errors.size) {
    console.log(`${errors.size} threw:`);
    for (const [p, m] of [...errors].slice(0, 5)) console.log(`  ${p}: ${m}`);
    if (errors.size > 5) console.log(`  ... and ${errors.size - 5} more`);
  }

  if (write || !existsSync(BASELINE)) {
    writeFileSync(BASELINE, formatBaseline(got));
    console.log(`${BASELINE} written, ${failing} differing`);
    return;
  }

  const { worse, better, gone } = compare(parseBaseline(readFileSync(BASELINE, 'utf8')), got);
  for (const b of better) console.log(`  improved: ${b.path} ${b.was} -> ${b.now}`);
  for (const g of gone) console.log(`  no longer in the corpus: ${g}`);
  if (!worse.length) {
    console.log('no regression against the baseline');
    return;
  }
  console.error(`\n${worse.length} regressed:`);
  for (const w of worse.slice(0, 20)) console.error(`  ${w.path}: ${w.was} -> ${w.now}`);
  if (worse.length > 20) console.error(`  ... and ${worse.length - 20} more`);
  console.error(`\nIf these are expected: npm run conformance -- -w`);
  process.exit(1);
}

const sel = process.argv.includes('-s') || process.argv.includes('--selftest');
await (sel ? selftest() : main());
