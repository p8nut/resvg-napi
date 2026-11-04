// Third-party notices for what the binaries actually contain.
//
//   npm run licenses    rewrite the notices from the crate graph; the diff
//                       shows what moved, and CI fails on one
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

const SNAPSHOT = 'THIRD-PARTY-NOTICES.md';

// Apache-2.0 and MIT ship beside this file; the rest have to be quoted here.
const ALREADY_SHIPPED = ['Apache-2.0', 'MIT'];

// The notice describes what a `.node` contains, so it has to ship in the
// packages that contain one. That is not the root package: the root ships
// JavaScript and type declarations, and every binary lives in one of the
// thirteen `npm/*` packages. Shipping the notice only from the root put it in
// the one package none of this code is in.
//
// LICENSE-APACHE and LICENSE-MIT travel with it because the notice names them
// for every crate it does not quote in full -- on their own, those crates'
// copyright lines would point at two files that are not there.
const BESIDE = ['LICENSE-APACHE', 'LICENSE-MIT'];

/** Copyright lines from a licence text, in order, deduplicated. */
export function copyrights(text) {
  const seen = new Set();
  for (const line of text.split('\n')) {
    const t = line.trim();
    // "Copyright (c) 2011 Google Inc." but not the Apache boilerplate line
    // "Copyright [yyyy] [name of copyright owner]", which names nobody, nor
    // the Apache prose that wraps onto a line starting "copyright notice that
    // is included..." or "copyright license to reproduce...". A holder's line
    // opens a sentence, so it is capitalised; the prose never is.
    if (/^C(opyright|OPYRIGHT)\b/.test(t) && !/\[yyyy\]|\[name of/i.test(t)) seen.add(t.replace(/\.$/, ''));
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

/**
 * The crates that can end up inside the binary: the root's normal
 * dependencies, transitively. `build-dependencies` are excluded -- autocfg,
 * version_check and napi-build run once and leave nothing behind, and calling
 * them statically linked would be false, which is the one thing a notice file
 * cannot afford to be.
 *
 * The proc-macro crates stay. Cargo files them under normal dependencies, and
 * that is the right answer here for a reason beyond bookkeeping: what a macro
 * emits is compiled into the binary. Over-listing costs a paragraph;
 * under-listing is the failure this file exists to prevent.
 *
 * Target-specific edges are kept whatever their target: thirteen platforms are
 * published from this graph, so the union is what ships.
 *
 * @param {{nodes: Array<{id: string, deps: Array<{pkg: string, dep_kinds: Array<{kind: string|null}>}>}>, root?: string}} resolve
 */
export function linkedIds(resolve, rootId) {
  const nodes = new Map(resolve.nodes.map((n) => [n.id, n]));
  const linked = new Set();
  const stack = [rootId];
  while (stack.length) {
    for (const dep of nodes.get(stack.pop())?.deps ?? []) {
      const normal = (dep.dep_kinds ?? []).some((k) => k.kind === null || k.kind === undefined);
      if (!normal || linked.has(dep.pkg)) continue;
      linked.add(dep.pkg);
      stack.push(dep.pkg);
    }
  }
  return linked;
}

function graph() {
  const out = execFileSync('cargo', ['metadata', '--format-version', '1', '--offline'],
    { encoding: 'utf8', maxBuffer: 64 * 1024 * 1024 });
  const meta = JSON.parse(out);
  const rootId = meta.resolve.root;
  const linked = linkedIds(meta.resolve, rootId);
  return meta.packages
    // `source` is null for crates built from this checkout -- the root and its
    // workspace members, e.g. crates/napi-support. Their licence is this
    // repository's, not a third party's.
    .filter((p) => p.id !== rootId && p.source !== null && linked.has(p.id))
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
    'The native binaries published from this repository statically link the Rust',
    'crates below. This file carries the notices their licences require to travel',
    'with a binary distribution, and ships in every package that holds one as well',
    'as in the root package. It is generated by `npm run licenses` from the crate',
    'graph in `Cargo.lock`; edit that, not this.',
    '',
    'The list is the root crate\'s normal dependency graph. Crates used only to',
    'build it are excluded. The proc-macro crates are not: what they emit is',
    'compiled in, so their terms travel with it.',
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

/** The `npm/*` packages: each holds a binary, so each carries the notice. */
function platformDirs() {
  if (!existsSync('npm')) return [];
  return readdirSync('npm')
    .map((d) => join('npm', d))
    .filter((dir) => existsSync(join(dir, 'package.json')));
}

if (import.meta.main) {
  const got = render(graph());
  writeFileSync(SNAPSHOT, got);
  console.log(`${SNAPSHOT} written, ${got.trim().split('\n').length} lines`);

  // What every package holding a `.node` must carry: the notice, and the two
  // licence texts it refers to by name rather than quoting. Nothing else
  // refreshes these copies -- `napi version` only moves the version field and
  // `napi artifacts` only drops the `.node` in -- so they are rewritten here
  // with the snapshot, and CI's diff catches a stale one the same way.
  //
  // The copy and its listing together: `files[]` is an allowlist, so a file
  // written into the directory and left off it ships nowhere. `napi
  // create-npm-dirs` rewrites those manifests from `napi.targets` and would
  // drop the three entries without saying so; this puts them back.
  const want = { [SNAPSHOT]: got };
  for (const f of BESIDE) want[f] = readFileSync(f, 'utf8');
  const dirs = platformDirs();
  for (const dir of dirs) {
    for (const [file, content] of Object.entries(want)) writeFileSync(join(dir, file), content);
    const manifest = join(dir, 'package.json');
    const m = JSON.parse(readFileSync(manifest, 'utf8'));
    const missing = Object.keys(want).filter((f) => !(m.files ?? []).includes(f));
    if (missing.length) {
      m.files = [...missing, ...(m.files ?? [])].sort();
      writeFileSync(manifest, `${JSON.stringify(m, null, 2)}\n`);
    }
  }
  console.log(`  and copied into ${dirs.length} platform packages`);
}
