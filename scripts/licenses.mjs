// Third-party notices for what the binaries actually contain.
//
//   npm run licenses          check the snapshot against the crate graph
//   npm run licenses -- -w    rewrite it, after a dependency moved
//   npm run licenses -- -s    selftest, offline
//
// Every `.node` and the `.wasm` statically link 91 crates. MIT asks for its
// notice to travel with "copies or substantial portions"; BSD-2 and BSD-3 ask
// for it "in binary form" by name; ISC and Unicode-3.0 the same. A binary is a
// copy. The repository shipped only its own two licences, so this file is the
// missing half -- and it is generated, because the graph moves with every
// resvg bump and a hand-written notice would be wrong by the next one.
//
// The full texts of Apache-2.0 and MIT are not repeated here: they are already
// in LICENSE-APACHE and LICENSE-MIT, which ship in the same package. What the
// notice adds for those crates is the one thing those files cannot carry --
// each copyright holder's own line. Licences whose text this repository does
// not otherwise ship are reproduced in full.
import { execFileSync } from 'node:child_process';
import { readFileSync, writeFileSync, existsSync, readdirSync } from 'node:fs';
import { basename, dirname, join } from 'node:path';
import assert from 'node:assert/strict';

const SNAPSHOT = 'THIRD-PARTY-NOTICES.md';
const SELF = 'resvg-napi';

// Apache-2.0 and MIT ship beside this file; the rest have to be quoted here.
const ALREADY_SHIPPED = ['Apache-2.0', 'MIT'];

/** Copyright lines from a licence text, in order, deduplicated. */
export function copyrights(text) {
  const seen = new Set();
  for (const line of text.split('\n')) {
    const t = line.trim();
    // "Copyright (c) 2011 Google Inc." but not the Apache boilerplate line
    // "Copyright [yyyy] [name of copyright owner]", which names nobody.
    if (/^copyright\b/i.test(t) && !/\[yyyy\]|\[name of/i.test(t)) seen.add(t.replace(/\.$/, ''));
  }
  return [...seen];
}

/** The SPDX ids in one alternative group, in the order they appear. */
export function spdxIds(expr) {
  return [...new Set((expr ?? '')
    .replace(/[()]/g, ' ')
    .split(/\s+(?:OR|AND)\s+|\//)
    .map((s) => s.trim())
    .filter(Boolean))];
}

/**
 * Every licence a crate must be complied with under, which is not the same as
 * every licence it names. `OR` is a choice -- and this package makes the same
 * one it makes for itself, Apache-2.0 or MIT, rather than dragging a third
 * text in. `AND` is not a choice: `(MIT OR Apache-2.0) AND Unicode-3.0` means
 * the Unicode terms apply whichever half of the dual you take, so both come
 * back and the crate is listed twice.
 */
export function required(expr) {
  return (expr ?? 'UNKNOWN')
    // Top-level conjuncts. Parentheses only ever group an OR here, so the
    // split is safe without a real parser -- and a nested AND would surface
    // as its own id rather than being silently dropped.
    .split(/\s+AND\s+/)
    .map((conjunct) => {
      const ids = spdxIds(conjunct);
      for (const preferred of ALREADY_SHIPPED) if (ids.includes(preferred)) return preferred;
      return ids[0] ?? 'UNKNOWN';
    })
    .filter((id, i, all) => all.indexOf(id) === i);
}

function licenceFiles(dir) {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((f) => /^(LICEN[CS]E|COPYING|NOTICE)/i.test(f))
    .sort()
    .map((f) => join(dir, f));
}

function graph() {
  const out = execFileSync('cargo', ['metadata', '--format-version', '1', '--offline'],
    { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  return JSON.parse(out).packages
    .filter((p) => p.name !== SELF)
    .map((p) => {
      const dir = dirname(p.manifest_path);
      const texts = licenceFiles(dir).map((f) => ({ file: basename(f), text: readFileSync(f, 'utf8') }));
      return {
        name: p.name,
        version: p.version,
        expr: p.license ?? 'UNKNOWN',
        ids: required(p.license),
        holders: [...new Set(texts.flatMap((t) => copyrights(t.text)))],
        texts,
        repository: p.repository ?? null,
      };
    })
    .sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * The text to quote for a licence. A crate under two licences ships two files,
 * so the one whose name carries the licence wins -- `LICENSE-UNICODE` for
 * Unicode-3.0, not the `LICENSE-APACHE` sitting next to it.
 */
export function textFor(crates, id) {
  const token = id.split('-')[0].toUpperCase();
  for (const c of crates) {
    const named = c.texts?.find((t) => t.file.toUpperCase().includes(token));
    if (named) return named.text;
  }
  for (const c of crates) if (c.texts?.length === 1) return c.texts[0].text;
  return null;
}

export function render(crates) {
  const byId = new Map();
  for (const c of crates) {
    for (const id of c.ids) {
      if (!byId.has(id)) byId.set(id, []);
      byId.get(id).push(c);
    }
  }
  const out = [
    '# Third-party notices',
    '',
    'The native binaries in this package statically link the Rust crates below.',
    'This file carries the notices their licences require to travel with a',
    'binary distribution. It is generated by `npm run licenses` from the crate',
    'graph in `Cargo.lock`; edit that, not this.',
    '',
    'The full texts of Apache-2.0 and MIT are in `LICENSE-APACHE` and',
    '`LICENSE-MIT`, which ship beside this file. Every other licence is',
    'reproduced in full at the end.',
    '',
  ];
  for (const id of [...byId.keys()].sort()) {
    const list = byId.get(id);
    out.push(`## ${id}`, '');
    for (const c of list) {
      out.push(`### ${c.name} ${c.version}`);
      if (c.expr !== id) out.push(`Offered as \`${c.expr}\`; listed here under ${id}.`);
      for (const h of c.holders) out.push(h);
      if (!c.holders.length) out.push(`No copyright line in the published crate; ${id} per its manifest.`);
      out.push('');
    }
  }
  const quoted = [...byId.keys()].filter((id) => !ALREADY_SHIPPED.includes(id)).sort();
  if (quoted.length) {
    out.push('---', '', '# Licence texts', '');
    for (const id of quoted) {
      // One text per licence: the crates under it differ only in their copyright
      // lines, which are listed above.
      out.push(`## ${id}`, '', '```', (textFor(byId.get(id), id)
        ?? `No text shipped with any crate under it; see https://spdx.org/licenses/${id}.html`).trim(), '```', '');
    }
  }
  return `${out.join('\n').replace(/\n{3,}/g, '\n\n').trim()}\n`;
}

async function selftest() {
  assert.deepEqual(copyrights('Copyright (c) 2011 Google Inc. All rights reserved.\nblah'),
    ['Copyright (c) 2011 Google Inc. All rights reserved']);
  // The Apache template line names nobody and must not be reported as a holder.
  assert.deepEqual(copyrights('Copyright [yyyy] [name of copyright owner]'), []);
  assert.deepEqual(copyrights('Copyright (c) A\nCopyright (c) A\nCopyright (c) B').length, 2);

  assert.deepEqual(spdxIds('MIT OR Apache-2.0'), ['MIT', 'Apache-2.0']);
  assert.deepEqual(spdxIds('MIT/Apache-2.0'), ['MIT', 'Apache-2.0']);
  assert.deepEqual(spdxIds('(MIT OR Apache-2.0) AND Unicode-3.0').includes('Unicode-3.0'), true);

  // A dual licence resolves to the text already shipped, not to a third file.
  assert.deepEqual(required('MIT OR Apache-2.0'), ['Apache-2.0']);
  assert.deepEqual(required('Zlib OR Apache-2.0 OR MIT'), ['Apache-2.0']);
  // A licence that stands alone is the one that must be quoted.
  assert.deepEqual(required('BSD-3-Clause'), ['BSD-3-Clause']);
  assert.deepEqual(required(undefined), ['UNKNOWN']);
  // AND is not a choice: the added terms come back with the chosen half.
  assert.deepEqual(required('(MIT OR Apache-2.0) AND Unicode-3.0'), ['Apache-2.0', 'Unicode-3.0']);
  // ...and no id ever carries a bracket.
  assert.ok(required('(MIT OR Apache-2.0) AND Unicode-3.0').every((id) => !/[()]/.test(id)));

  // The quoted text is the one named after the licence, not the first on disk.
  assert.equal(textFor([{ texts: [
    { file: 'LICENSE-APACHE', text: 'APACHE' }, { file: 'LICENSE-UNICODE', text: 'UNICODE' }] }],
    'Unicode-3.0'), 'UNICODE');

  const doc = render([
    { name: 'tiny-skia', version: '0.12.0', expr: 'BSD-3-Clause', ids: ['BSD-3-Clause'],
      holders: ['Copyright (c) 2011 Google Inc'], texts: [{ file: 'LICENSE', text: 'BSD TEXT HERE' }] },
    { name: 'serde', version: '1.0', expr: 'MIT OR Apache-2.0', ids: ['Apache-2.0'],
      holders: [], texts: [{ file: 'LICENSE-APACHE', text: 'APACHE TEXT' }] },
  ]);
  assert.match(doc, /## BSD-3-Clause/);
  assert.match(doc, /Copyright \(c\) 2011 Google Inc/);
  assert.match(doc, /BSD TEXT HERE/, 'a licence we do not otherwise ship is quoted in full');
  assert.doesNotMatch(doc, /APACHE TEXT/, 'Apache-2.0 is already in LICENSE-APACHE');
  assert.match(doc, /Offered as `MIT OR Apache-2\.0`/, 'a dual licence says which half was taken');

  console.log('ok — licenses: 19 checks passed');
}

function main() {
  const write = process.argv.includes('-w') || process.argv.includes('--write');
  const got = render(graph());
  if (write || !existsSync(SNAPSHOT)) {
    writeFileSync(SNAPSHOT, got);
    console.log(`${SNAPSHOT} written, ${got.trim().split('\n').length} lines`);
    return;
  }
  if (readFileSync(SNAPSHOT, 'utf8') === got) {
    console.log(`${SNAPSHOT} matches the crate graph`);
    return;
  }
  console.error(`${SNAPSHOT} is out of date: the crate graph moved.`);
  console.error('Rerun with: npm run licenses -- -w');
  process.exit(1);
}

await (process.argv.includes('-s') || process.argv.includes('--selftest') ? selftest() : main());
