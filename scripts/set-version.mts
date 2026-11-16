// Writes a release version everywhere it lives: `node scripts/set-version.mts 1.2.3`.
// The repository stays at 0.0.0; CI runs this on a `v*` tag, with the tag's
// version, before it builds, packs or publishes (CONTRIBUTING.md, Releasing).
// Nothing it writes is committed.
import { readFileSync, readdirSync, writeFileSync } from 'node:fs';

const version = process.argv[2];
if (!/^\d+\.\d+\.\d+(-[0-9A-Za-z.-]+)?$/.test(version ?? '')) {
  console.error('usage: node scripts/set-version.mts X.Y.Z');
  process.exit(1);
}
/** The fields of a package.json this script reads and writes. */
interface Manifest {
  name: string
  version: string
  optionalDependencies?: Record<string, string>
  peerDependencies?: Record<string, string>
  dependencies?: Record<string, string>
}

const text = (file: string, edit: (s: string) => string) => writeFileSync(file, edit(readFileSync(file, 'utf8')));
const json = (file: string, edit: (m: Manifest) => void) => text(file, (s) => {
  const m: Manifest = JSON.parse(s);
  edit(m);
  return `${JSON.stringify(m, null, 2)}\n`;
});

let old = '';
let name = '';
json('package.json', (m) => {
  ({ version: old, name } = m);
  m.version = version;
  // The root pins its platform packages, the wasm one as a peer too.
  for (const deps of [m.optionalDependencies, m.peerDependencies, m.dependencies]) {
    for (const dep of Object.keys(deps ?? {})) if (dep.startsWith(`${name}-`)) deps![dep] = version;
  }
});
for (const dir of readdirSync('npm')) json(`npm/${dir}/package.json`, (m) => { m.version = version; });
// The crate version is hashed into every class's napi type tag: two releases
// sharing it accept each other's objects (CONTRIBUTING.md).
text('Cargo.toml', (s) => s.replace(/^version = ".*"/m, `version = "${version}"`));
text('Cargo.lock', (s) => s.replace(`name = "${name}"\nversion = "${old}"`, `name = "${name}"\nversion = "${version}"`));
// What `napi build` would write into index.js's binding checks.
text('index.js', (s) => s.replaceAll(`'${old}'`, `'${version}'`).replaceAll(`expected ${old} but`, `expected ${version} but`));
console.log(`${old} -> ${version}`);
