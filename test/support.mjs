// Fonts for the tests. Three environments, three answers:
//
//   native on a desktop  -- loadSystemFonts() finds hundreds
//   native in a container -- may find none, so a file is read instead
//   wasm32-wasip1-threads -- never finds any: WASI has no font directories
//
// Nothing here hardcodes a family name. `family` is whatever the database
// actually holds, which is the only name `fontFamily` will resolve.
import { existsSync, readFileSync } from 'node:fs';

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
 * A database with at least one face, and the family name to ask for.
 * Returns `null` when the environment has no font at all, so a test can say
 * what it skipped instead of failing on the environment.
 */
export function testFonts(FontDatabase) {
  const db = new FontDatabase();
  db.loadSystemFonts();
  if (!db.len()) {
    const file = CANDIDATES.find((f) => existsSync(f));
    if (!file) return null;
    db.loadFontData(readFileSync(file));
  }
  // The first system face is whatever fontdb happened to enumerate first
  // (Comfortaa, on this machine): fine as a name, risky for glyph coverage.
  // Prefer a workhorse when one is installed.
  const families = new Set(db.faces().flatMap((f) => f.families));
  const family = ['DejaVu Sans', 'Liberation Sans', 'Arial', 'Helvetica', 'Segoe UI']
    .find((name) => families.has(name)) ?? db.faces()[0].families[0];
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
 * Prints what was not checked, and why.
 *
 * Seven of the fourteen test files skip themselves when no font is present, and
 * a skip leaves the exit code at 0 -- so half the suite could go quiet and the
 * runner would still report fourteen passes. That is right for WASI, which has
 * no font directories by design, and wrong everywhere else.
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
