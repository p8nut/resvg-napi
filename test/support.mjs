// Fonts for the tests. Three environments, three answers:
//
//   native on a desktop  -- loadSystemFonts() finds hundreds
//   native in a container -- may find none, so a file is read instead
//   wasm32-wasip1-threads -- never finds any: WASI has no font directories
//
// Nothing here hardcodes a family name. `family` is whatever the database
// actually holds, which is the only name `fontFamily` will resolve.
//
// There is always a file to read: the repo ships IBM Plex Sans (Latin) for the
// demo, the last candidate. Every fixture's text is ASCII, which it covers, and
// the whole suite passes with it as the only face. So nothing skips.
import { existsSync, readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { nodeExtent } from '../helpers.browser.mjs';

const CANDIDATES = [
  process.env.RESVG_TEST_FONT,
  'demo/DejaVuSans.ttf',
  '/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf',
  '/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf',
  '/usr/share/fonts/TTF/DejaVuSans.ttf',
  '/System/Library/Fonts/Supplemental/Arial.ttf',
  'C:/Windows/Fonts/arial.ttf',
  // Absolute, so it resolves whatever the working directory.
  fileURLToPath(new URL('../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url)),
].filter(Boolean);

/**
 * A `fontdb.Database` with at least one face, and the family name to ask
 * for: the system's fonts, else a font file.
 */
export function testFonts({ fontdb }) {
  const db = new fontdb.Database();
  db.loadSystemFonts();
  if (!db.len()) db.loadFontData(readFileSync(testFontFile()));
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

/** The font file the tests can load themselves: the repo's own as a last resort. */
export function testFontFile() {
  return CANDIDATES.find((f) => existsSync(f));
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
