// Whether a pull request moves the public API, read off `index.d.ts`.
//
// The surface mirrors upstream (ADR 0001), so any bump of resvg, usvg, fontdb
// or tiny-skia can change it, and a reviewer should not have to read a
// 2000-line generated diff to know. This keys every declaration of two
// `index.d.ts` texts as `namespace.container.member`, with a normalised
// signature, and classifies the difference:
//
//   none       no key added, removed or changed (doc comments do not count)
//   additive   only additions: new keys, a trailing optional parameter, a
//              union alias that gained members, an interface merged into a
//              class of the same name
//   breaking   anything else: a key removed (a move between namespaces is a
//              removal), a signature changed, a union member dropped, a
//              required field added to an existing interface
//
// Changed is breaking unless one of those three widenings explains it. That is
// conservative on purpose: a new required field on an interface breaks
// whoever builds that object, a widened return type breaks whoever switches on
// it, and the d.ts does not say which way a type flows.
//
// napi's output is deterministic and one member per line, which is why a line
// scanner is enough and the TypeScript compiler is not a dependency.
//
//   node scripts/api-diff.mjs <base.d.ts> <head.d.ts>
//
// prints a markdown summary, then `classification=<none|additive|breaking>` as
// its last line. `.github/workflows/api.yml` and `resvg.yml` read both.

import { readFileSync } from 'node:fs';

/** Drop `/* *\/` and `//` comments, leaving string literals alone. */
export function stripComments(text) {
  let out = '';
  for (let i = 0; i < text.length; ) {
    const c = text[i];
    if (c === "'" || c === '"' || c === '`') {
      let j = i + 1;
      while (j < text.length && text[j] !== c) j += text[j] === '\\' ? 2 : 1;
      out += text.slice(i, j + 1);
      i = j + 1;
    } else if (c === '/' && text[i + 1] === '*') {
      const end = text.indexOf('*/', i + 2);
      // Keep the line count: nothing depends on it, but a stray newline inside
      // a comment must not glue two declarations onto one line.
      const body = text.slice(i, end < 0 ? text.length : end + 2);
      out += body.replace(/[^\n]/g, '');
      i = end < 0 ? text.length : end + 2;
    } else if (c === '/' && text[i + 1] === '/') {
      while (i < text.length && text[i] !== '\n') i++;
    } else {
      out += c;
      i++;
    }
  }
  return out;
}

/** Bracket depth change of a line, outside string literals. */
function depth(line) {
  let d = 0;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === "'" || c === '"' || c === '`') {
      let j = i + 1;
      while (j < line.length && line[j] !== c) j += line[j] === '\\' ? 2 : 1;
      i = j;
    } else if ('([{<'.includes(c)) d++;
    else if (')]}'.includes(c)) d--;
    else if (c === '>' && line[i - 1] !== '=') d--;
  }
  return d;
}

/** Whitespace collapsed: what a reader sees. */
const tidy = (s) => s.replace(/\s+/g, ' ').trim().replace(/[;,]$/, '').trim();

/** Whitespace gone around punctuation: what two signatures are compared by. */
const compact = (s) => tidy(s).replace(/\s*([()[\]{}<>,:;?|&=])\s*/g, '$1');

/** Split on `sep` at bracket depth zero, outside string literals. */
function splitTop(s, sep) {
  const parts = [];
  let d = 0;
  let start = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s[i];
    if (c === "'" || c === '"' || c === '`') {
      let j = i + 1;
      while (j < s.length && s[j] !== c) j += s[j] === '\\' ? 2 : 1;
      i = j;
    } else if ('([{<'.includes(c)) d++;
    else if (')]}'.includes(c)) d--;
    else if (c === '>' && s[i - 1] !== '=') d--;
    else if (d === 0 && c === sep) {
      parts.push(s.slice(start, i));
      start = i + 1;
    }
  }
  parts.push(s.slice(start));
  return parts.map((p) => p.trim()).filter((p) => p !== '');
}

const PREFIX = /^(?:export\s+)?(?:declare\s+)?/;
const CONTAINER =
  /^(?:export\s+)?(?:declare\s+)?(class|interface|namespace|module|const\s+enum|enum|abstract\s+class)\s+([\w$]+)(.*)\{\s*$/;

/**
 * Every declaration of a d.ts text, as key -> signature.
 *
 * A container (class, interface, enum, namespace) has a key of its own, and
 * its members hang under it. Two declarations on one key -- an overload, or
 * the interface napi merges into a class of the same name -- share it, their
 * signatures joined in a stable order.
 */
export function declarations(text) {
  const lines = stripComments(text).split('\n');
  /** @type {Map<string, string[]>} */
  const found = new Map();
  const add = (key, sig) => found.set(key, [...(found.get(key) ?? []), tidy(sig)]);
  /** @type {{name: string, kind: string}[]} */
  const stack = [];
  const path = (name) => [...stack.map((s) => s.name), name].join('.');

  for (let i = 0; i < lines.length; i++) {
    let line = lines[i].trim();
    if (line === '' || /^import\s/.test(line)) continue;
    if (line === '}' || line === '};') {
      stack.pop();
      continue;
    }

    const head = line.match(CONTAINER);
    if (head) {
      const kind = head[1].replace(/\s+/g, ' ');
      add(path(head[2]), `${kind}${head[3] ? ' ' + head[3] : ''}`);
      stack.push({ name: head[2], kind });
      continue;
    }

    // A statement spans lines until its brackets close and the next line does
    // not carry on a union: napi never does either, but a hand-edited d.ts may.
    let d = depth(line);
    while (i + 1 < lines.length && (d > 0 || /^\s*\|/.test(lines[i + 1]))) {
      const next = lines[++i];
      line += ' ' + next.trim();
      d += depth(next);
    }

    const top = stack.at(-1);
    if (!top || top.kind === 'namespace' || top.kind === 'module') {
      const body = line.replace(PREFIX, '');
      let m;
      if ((m = body.match(/^function\s+([\w$]+)\s*(.*)$/))) add(path(m[1]), `function ${m[2]}`);
      else if ((m = body.match(/^(const|let|var)\s+([\w$]+)\s*(.*)$/))) add(path(m[2]), `${m[1]} ${m[3]}`);
      else if ((m = body.match(/^type\s+([\w$]+)\s*(.*)$/))) add(path(m[1]), `type ${m[2]}`);
      // Anything this scanner does not know is kept whole, so a change to it
      // still shows instead of slipping through as `none`.
      else add(path(`<${tidy(body)}>`), 'statement');
    } else if (top.kind.endsWith('enum')) {
      const m = line.match(/^([\w$]+|'[^']*'|"[^"]*")\s*(.*)$/);
      add(path(m[1]), m[2] || 'member');
    } else {
      // static / readonly / get / set, then the name, then the rest.
      const m = line.match(
        /^(static\s+)?(readonly\s+)?(?:(get|set)\s+(?=[\w$[]))?(\[[^\]]+\]|[\w$]+)(\?)?\s*(.*)$/,
      );
      if (!m) {
        add(path(`<${tidy(line)}>`), 'member');
        continue;
      }
      const [, isStatic, isReadonly, accessor, name, optional, rest] = m;
      const key = `${isStatic ? 'static ' : ''}${accessor ? accessor + ' ' : ''}${name}`;
      add(path(key), `${isReadonly ? 'readonly ' : ''}${optional ?? ''}${rest}`);
    }
  }

  return new Map([...found].map(([k, sigs]) => [k, [...new Set(sigs)].sort().join(' & ')]));
}

/** `(params)rest` taken apart, or null when the signature is not a call. */
function call(sig) {
  const s = sig.replace(/^function\s+/, '');
  if (!s.startsWith('(')) return null;
  let d = 0;
  for (let i = 0; i < s.length; i++) {
    if (s[i] === '(') d++;
    else if (s[i] === ')' && --d === 0) {
      return { params: splitTop(s.slice(1, i), ','), rest: compact(s.slice(i + 1)) };
    }
  }
  return null;
}

/** The members of a string-literal (or any) union alias, or null. */
function union(sig) {
  const m = sig.match(/^type\s*=\s*(.*)$/);
  return m ? splitTop(m[1], '|').map(compact) : null;
}

/**
 * Why a changed signature is additive, or null when it is breaking.
 *
 * Three widenings only: optional parameters appended after the old ones, an
 * interface or namespace merged into a declaration of the same name, and a
 * union alias that kept every member it had.
 */
export function widening(base, head) {
  const [b, h] = [call(base), call(head)];
  if (b && h && b.rest === h.rest && h.params.length > b.params.length) {
    const kept = b.params.every((p, i) => compact(p) === compact(h.params[i]));
    const extra = h.params.slice(b.params.length);
    if (kept && extra.every((p) => /^[\w$]+\?\s*:/.test(p) || p.startsWith('...'))) {
      return `optional parameter added: ${extra.join(', ')}`;
    }
  }
  // `interface Resvg` merged into `class Resvg`: what it brings is keyed
  // member by member, so the merge itself takes nothing away.
  const [kb, kh] = [base.split(' & '), head.split(' & ')];
  const kinds = /^(abstract class|class|interface|namespace|module|const enum|enum)\b/;
  if ([...kb, ...kh].every((k) => kinds.test(k)) && kb.every((k) => kh.includes(k))) {
    const extra = kh.filter((k) => !kb.includes(k));
    if (extra.every((k) => /^(interface|namespace)\b/.test(k))) {
      return `declaration merged: ${extra.join(', ')}`;
    }
  }
  const [ub, uh] = [union(base), union(head)];
  if (ub && uh && uh.length > ub.length && ub.every((m) => uh.includes(m))) {
    return `union widened: ${uh.filter((m) => !ub.includes(m)).join(' | ')}`;
  }
  return null;
}

/**
 * The difference between two d.ts texts.
 *
 * @returns {{classification: 'none'|'additive'|'breaking',
 *   added: string[], removed: string[], required: string[],
 *   changed: {key: string, base: string, head: string, widening: string|null}[],
 *   before: Map<string,string>, after: Map<string,string>}}
 */
export function apiDiff(baseText, headText) {
  const before = declarations(baseText);
  const after = declarations(headText);
  const added = [...after.keys()].filter((k) => !before.has(k)).sort();
  const removed = [...before.keys()].filter((k) => !after.has(k)).sort();
  const changed = [...before.keys()]
    .filter((k) => after.has(k) && compact(before.get(k)) !== compact(after.get(k)))
    .sort()
    .map((key) => ({
      key,
      base: before.get(key),
      head: after.get(key),
      widening: widening(before.get(key), after.get(key)),
    }));
  // A required field added to an interface that already existed breaks every
  // object literal of that type; on a class, a new member breaks nobody.
  const required = added.filter((k) => {
    const parent = k.slice(0, k.lastIndexOf('.'));
    const kind = before.get(parent) ?? '';
    return /^interface\b/.test(kind) && !/^(readonly )?\?/.test(after.get(k));
  });
  const classification =
    removed.length || required.length || changed.some((c) => !c.widening)
      ? 'breaking'
      : added.length || changed.length
        ? 'additive'
        : 'none';
  return { classification, added, removed, required, changed, before, after };
}

/**
 * A key whose container is in the same list goes without saying: a removed
 * class lists once, not once per member.
 */
function outermost(keys) {
  const set = new Set(keys);
  return keys.filter((k) => {
    const parts = k.split('.');
    return !parts.slice(1).some((_, i) => set.has(parts.slice(0, i + 1).join('.')));
  });
}

const LIMIT = 60;
const WIDTH = 160;

/**
 * Inline code, safe for a PR comment: everything here comes from a pull
 * request's d.ts, so it stays inside a code span (no mention, no link, no
 * markup) and is cut to length.
 */
function code(s) {
  const t = s.replace(/`/g, "'").replace(/\s+/g, ' ');
  return '`' + (t.length > WIDTH ? t.slice(0, WIDTH - 1) + '…' : t) + '`';
}

function section(title, lines) {
  if (!lines.length) return [];
  const shown = lines.slice(0, LIMIT);
  const more = lines.length - shown.length;
  return [`**${title}** (${lines.length})`, '', ...shown, ...(more > 0 ? [`- ...and ${more} more`] : []), ''];
}

/** The markdown summary, without the machine line. */
export function summary(diff) {
  const { classification, added, removed, required, changed, before, after } = diff;
  const verdict = {
    none: 'no change to the public API',
    additive: 'additions only',
    breaking: 'removals or changed signatures',
  }[classification];
  const out = [`### \`index.d.ts\`: ${classification}`, '', `${verdict[0].toUpperCase()}${verdict.slice(1)}.`, ''];
  if (classification === 'none') return out.join('\n');
  const breaking = changed.filter((c) => !c.widening);
  const widened = changed.filter((c) => c.widening);
  out.push(
    ...section(
      'Removed',
      outermost(removed).map((k) => `- ${code(k)} ${code(before.get(k))}`),
    ),
    ...section(
      'Changed',
      breaking.map((c) => `- ${code(c.key)}: ${code(c.base)} → ${code(c.head)}`),
    ),
    ...section(
      'Required on an existing interface',
      required.map((k) => `- ${code(k)} ${code(after.get(k))}`),
    ),
    ...section(
      'Widened',
      widened.map((c) => `- ${code(c.key)}: ${code(c.widening)}`),
    ),
    ...section(
      'Added',
      outermost(added.filter((k) => !required.includes(k))).map((k) => `- ${code(k)} ${code(after.get(k))}`),
    ),
  );
  return out.join('\n');
}

if (import.meta.main) {
  const [basePath, headPath] = process.argv.slice(2);
  if (!basePath || !headPath) {
    console.error('usage: node scripts/api-diff.mjs <base.d.ts> <head.d.ts>');
    process.exit(2);
  }
  const diff = apiDiff(readFileSync(basePath, 'utf8'), readFileSync(headPath, 'utf8'));
  process.stdout.write(`${summary(diff)}\nclassification=${diff.classification}\n`);
}
