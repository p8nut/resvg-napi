// Drops the `export type UsvgTree = Tree` lines `napi build` writes into a
// `.d.ts`: `node scripts/dts-aliases.mts [file..]`, by default `index.d.ts`
// and `resvg-napi.wasi.d.cts` where they exist. Run after every `napi build`
// (package.json, CI), the toy's included.
//
// napi aliases each class's Rust identifier to its JS name, unconditionally
// (napi-derive's typegen always records it, the CLI prints it whenever the
// two differ). The generator prefixes those identifiers with the crate
// (`UsvgTree`), because napi keys a class by its Rust identifier and one
// generated file holds every namespace (ADR 0002, Names): so every class
// came with an alias naming that prefix, which is invisible from JS
// everywhere else. Nothing reads the aliases; `scripts/api-diff.mjs`
// ignores them.
import { existsSync, readFileSync, writeFileSync } from 'node:fs';

/**
 * `text` without the alias napi prints on the line after a class's closing
 * brace, naming that class: `export type <RustName> = <JsName>`.
 */
export function dropAliases(text: string): string {
  const lines = text.split('\n');
  const out: string[] = [];
  let open = '';
  for (const line of lines) {
    const head = line.match(/^\s*export (?:declare )?class ([\w$]+)\b/);
    if (head) open = head[1];
    const alias = line.match(/^(\s*)export type [\w$]+ = ([\w$]+)$/);
    if (alias && alias[2] === open && out.at(-1) === `${alias[1]}}`) continue;
    out.push(line);
  }
  return out.join('\n');
}

if (import.meta.main) {
  const given = process.argv.slice(2);
  const files = given.length ? given : ['index.d.ts', 'resvg-napi.wasi.d.cts'].filter((f) => existsSync(f));
  for (const f of files) {
    const text = readFileSync(f, 'utf8');
    const kept = dropAliases(text);
    if (kept !== text) writeFileSync(f, kept);
  }
}
