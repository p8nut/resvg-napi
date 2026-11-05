#!/usr/bin/env node
// Headless twin of the bench: Liquid with the same filter vocabulary, then
// `resvg-napi/helpers` on the native binding. `node demo/render.mjs
// examples/badge-sheet.svg` proves an example still renders without opening a
// browser.
//
//   node demo/render.mjs <template.svg> [out.png]
//
// Variables come from `<template>.json` when it exists, partials from files
// next to the template, fonts from `demo/fonts` plus `demo/DejaVuSans.ttf`.
import { readFileSync, existsSync, writeFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve, basename } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { Liquid } from 'liquidjs';
// The package by its own name, as a user imports it: `resvg-napi` for the
// binding (fonts, logs), `resvg-napi/helpers` for parsing and rendering.
import binding from 'resvg-napi';
import { parse, renderPng } from 'resvg-napi/helpers';
import { measureText, registerSvgFilters, referencedPartials } from './svg-filters.mjs';

// fileURLToPath, not `.pathname`: on Windows the latter yields
// `/D:/a/...`, and join() then builds `\D:\a\...` -- a leading separator
// before the drive letter, which resolves to nothing.
const here = dirname(fileURLToPath(import.meta.url));

// `--lang fr` drives `<switch systemLanguage>`; usvg picks nothing without it.
// `--strict` is a flag, not an environment variable: `VAR=1 node ...` is POSIX
// shell syntax and `cmd` on the Windows runner answers "'VAR' is not recognized
// as an internal or external command", which is how the 0.2.0 tag failed its
// test job.
const { values: flags, positionals: [templateArg, outArg] } = parseArgs({
  allowPositionals: true,
  options: { lang: { type: 'string' }, strict: { type: 'boolean' } },
});
const languages = flags.lang === undefined ? undefined : [flags.lang];
const strict = flags.strict ?? false;
if (!templateArg) {
  console.error('usage: node demo/render.mjs <template.svg> [out.png] [--lang xx] [--strict]');
  process.exit(2);
}
const template = resolve(here, templateArg);
const out = outArg ? resolve(outArg) : template.replace(/\.svg$/, '.png');

binding.setLogLevel('warn');

// One database for everything: rendering, and measuring for the filters.
const db = new binding.fontdb.Database();
const fontFiles = [join(here, 'DejaVuSans.ttf'),
  ...(existsSync(join(here, 'fonts')) ? readdirSync(join(here, 'fonts'))
    .filter((f) => /\.(ttf|otf)$/i.test(f)).map((f) => join(here, 'fonts', f)) : [])]
  .filter(existsSync);
let family;
for (const file of fontFiles) {
  const loaded = db.len();
  db.loadFontData(readFileSync(file));
  // What fontdb calls the file's first face: `families[0]`, a
  // `[name, language]` pair, English (US) first.
  family ??= db.faces()[loaded]?.families[0]?.[0];
}
// Local files first, so the generic families below point at a known face; then
// the system, for the glyphs the local files do not have (emoji, scripts).
db.loadSystemFonts();
// The examples ask for `sans-serif`; point every generic family at what we have.
for (const set of ['setSansSerifFamily', 'setSerifFamily', 'setMonospaceFamily',
                   'setCursiveFamily', 'setFantasyFamily']) {
  db[set](family);
}
const options = { fontFamily: family, languages };

const measure = (text, opts) => measureText(binding, db, text, opts, options);

// Output lands inside XML, so every `{{ }}` is escaped; a filter that returns
// markup (`qr`, `wrap`) is piped through `raw` in the template.
const liquid = new Liquid({
  root: dirname(template), extname: '', relativeReference: false, outputEscape: 'escape',
});
registerSvgFilters(liquid, { measure });

const source = readFileSync(template, 'utf8');
const varsFile = template.replace(/\.svg$/, '.json');
const scope = existsSync(varsFile) ? JSON.parse(readFileSync(varsFile, 'utf8')) : {};

const wanted = [...referencedPartials(liquid, source)];

const rendered = liquid.parseAndRenderSync(source, scope);
// An unrendered tag means the template asked for something Liquid did not do,
// which is a failure and not a warning: resvg would parse the tag as markup.
// A `{% raw %}` block emits braces on purpose, so that check cannot apply.
const emitsLiteralBraces = /\{%-?\s*raw\s*-?%\}/.test(source);
const left = rendered.match(/\{[{%]/g);
if (left && !emitsLiteralBraces) {
  console.error(`${left.length} unrendered Liquid tag(s) left in the output`);
  process.exit(1);
}
if (left && emitsLiteralBraces) {
  console.log(`  note: ${left.length} literal brace(s) from a {% raw %} block, not checked`);
}

// The helpers parse through `guard`'s resolvers: an `<image href>` resolves
// from the document (`data:`) and nothing is read from disk, and what did not
// resolve comes back in `pendingImages` / `pendingFonts`.
const { tree, pendingImages, pendingFonts } = parse(rendered, { ...options, fonts: db });
writeFileSync(out, renderPng(tree, { scale: 2 }));
const size = tree.size();

console.log(`${basename(template)}: ${size.width()}×${size.height()} units`
  + `${wanted.length ? `, partials [${wanted.join(', ')}]` : ''}`
  + `${Object.keys(scope).length ? `, vars [${Object.keys(scope).join(', ')}]` : ''} → ${out}`);
const logs = binding.takeLogs();
for (const line of logs) console.log('  ' + line);

// `npm run test:examples` runs this over four documents, and it used to pass
// whatever the renderer said: a font that stopped resolving, a shape usvg
// skipped, an href it could not read -- all printed, none fatal. The examples
// render clean today, so anything usvg reports is a regression.
//
// Off by default: rendering a document of your own and being told about its
// problems is the point of this script. `--strict` is what `test:examples`
// passes.
if (strict) {
  // Which fonts a machine has is an environment fact, not a regression, and usvg
  // says so in two ways: it cannot match a family at all, or it matches a
  // different one. photo-card.svg asks for an emoji font -- the Linux runner
  // ships Noto Color Emoji, Windows falls back to Segoe UI Emoji, macOS has
  // neither. Both phrasings are reported and neither is fatal.
  //
  // Deliberately not "ignore everything from usvg::text": a shaping failure or a
  // malformed `font-size` comes from the same module and is a defect. These are
  // the two font-*selection* messages, matched as written.
  const environmental = /usvg::text: (No match for .* font-family|Fallback from .* to )/;
  const defects = logs.filter((l) => !environmental.test(l));
  if (pendingFonts.length) {
    console.log(`  note: ${pendingFonts.length} font(s) this machine does not have: ${pendingFonts.join(', ')}`);
  }
  const why = [
    defects.length && `${defects.length} diagnostic(s) from usvg`,
    pendingImages.length && `unresolved image(s): ${pendingImages.join(', ')}`,
  ].filter(Boolean);
  if (why.length) {
    console.error(`  FAIL ${basename(template)}: ${why.join('; ')}`);
    process.exit(1);
  }
}
