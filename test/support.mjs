// Fonts for the tests. Three environments, three answers:
//
//   native on a desktop  -- loadSystemFonts() finds hundreds
//   native in a container -- may find none, so a file is read instead
//   wasm32-wasip1-threads -- never finds any: WASI has no font directories
//
// Nothing here hardcodes a family name. `family` is whatever the database
// actually holds, which is the only name `fontFamily` will resolve.
import { existsSync, readFileSync } from 'node:fs';
import { nodeExtent } from '../helpers.browser.mjs';

const CANDIDATES = [
  process.env.RESVG_TEST_FONT,
  'demo/DejaVuSans.ttf',
  '/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf',
  '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf',
  '/usr/share/fonts/TTF/DejaVuSans.ttf',
  '/System/Library/Fonts/Supplemental/Arial.ttf',
  'C:/Windows/Fonts/arial.ttf',
].filter(Boolean);

/**
 * A `fontdb.Database` with at least one face, and the family name to ask
 * for. Returns `null` when the environment has no font at all, so a test can
 * say what it skipped instead of failing on the environment.
 */
export function testFonts({ fontdb }) {
  const db = new fontdb.Database();
  db.loadSystemFonts();
  if (!db.len()) {
    const file = testFontFile();
    if (!file) return null;
    db.loadFontData(readFileSync(file));
  }
  // The first system face is whatever fontdb happened to enumerate first
  // (Comfortaa, on this machine): fine as a name, risky for glyph coverage.
  // Prefer a workhorse when one is installed. `families` is `[name,
  // language]` pairs.
  const faces = db.faces();
  const families = new Set(faces.flatMap((f) => f.families.map(([name]) => name)));
  const family = ['DejaVu Sans', 'Liberation Sans', 'Arial', 'Helvetica', 'Segoe UI']
    .find((name) => families.has(name)) ?? faces[0].families[0][0];
  // The generic families point at it too, so `font-family="sans-serif"` in a
  // fixture resolves wherever the test runs.
  for (const set of ['setSansSerifFamily', 'setSerifFamily', 'setMonospaceFamily',
                     'setCursiveFamily', 'setFantasyFamily']) db[set](family);
  return { db, family };
}

/** The font file the tests can load themselves, or null if there is none. */
export function testFontFile() {
  return CANDIDATES.find((f) => existsSync(f)) ?? null;
}

/**
 * `testFonts`, or the end of the test file: when the environment has no font,
 * says what was skipped (see `skip`) and exits 0.
 */
export function fontsOrSkip(binding, what) {
  const fonts = testFonts(binding);
  if (!fonts) {
    skip(what, 'no font in this environment');
    process.exit(0);
  }
  return fonts;
}

/** Where a `usvg.Node` will draw (the helpers' `nodeExtent`). */
export const extent = nodeExtent;

/**
 * One node alone, in a pixmap the size of its extent: `resvg.renderNode`
 * draws the node's local geometry offset by its layer box, so both are
 * undone. The layer box is a group's own, and a path's fill box. A group's
 * own transform is applied by `renderNode`, so only its parent's is passed.
 */
export function renderNodePng({ resvg, tiny_skia }, node, { scale = 1 } = {}) {
  const box = extent(node);
  const group = node.kind === 'group';
  const fill = node.value.absBoundingBox();
  const inner = group ? box : { x: fill.x(), y: fill.y() };
  const parent = group
    ? node.value.absTransform().preConcat(node.value.transform().invert())
    : node.value.absTransform();
  const pixmap = new tiny_skia.Pixmap(Math.ceil(box.width * scale), Math.ceil(box.height * scale));
  const transform = tiny_skia.Transform.fromScale(scale, scale)
    .preTranslate(-box.x, -box.y)
    .preConcat(parent)
    .preTranslate(inner.x, inner.y);
  if (!resvg.renderNode(node, transform, pixmap)) throw new Error('element is empty');
  return pixmap.encodePng();
}

/**
 * Prints what was not checked, and why.
 *
 * Several test files skip themselves when no font is present, and a skip
 * leaves the exit code at 0 -- so half the suite could go quiet and the runner
 * would still report every file passed. That is right for WASI, which has no
 * font directories by design, and wrong everywhere else.
 *
 * `RESVG_REQUIRE_FONTS=1` turns a skip into a failure. CI sets it on the native
 * jobs, where a missing font means the environment broke rather than that it is
 * a sandbox.
 */
export function skip(what, why) {
  if (process.env.RESVG_REQUIRE_FONTS === '1') {
    console.error(`FAIL — ${what}: ${why} (RESVG_REQUIRE_FONTS=1)`);
    process.exit(1);
  }
  console.log(`skip — ${what}: ${why}`);
}
