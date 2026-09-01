#!/usr/bin/env node
// Headless twin of the bench: Liquid with the same filter vocabulary, then the
// native binding. `node demo/render.mjs examples/badge-sheet.svg` proves an
// example still renders without opening a browser.
//
//   node demo/render.mjs <template.svg> [out.png]
//
// Variables come from `<template>.json` when it exists, partials from files
// next to the template, fonts from `demo/fonts` plus `demo/DejaVuSans.ttf`.
import { readFileSync, existsSync, writeFileSync, readdirSync } from 'node:fs';
import { dirname, join, resolve, basename } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';
import { Liquid } from 'liquidjs';
import { registerSvgFilters, referencedPartials } from './svg-filters.mjs';
import { qrSvg } from './liquid-entry.mjs';

// fileURLToPath, not `.pathname`: on Windows the latter yields
// `/D:/a/...`, and join() then builds `\D:\a\...` -- a leading separator
// before the drive letter, which resolves to nothing.
const here = dirname(fileURLToPath(import.meta.url));
const require = createRequire(join(here, '..', 'index.js'));
const mod = require(join(here, '..', 'index.js'));

const argv = process.argv.slice(2);
// `--lang fr` drives `<switch systemLanguage>`; usvg picks nothing without it.
const langAt = argv.indexOf('--lang');
const languages = langAt === -1 ? undefined : [argv[langAt + 1]];
if (langAt !== -1) argv.splice(langAt, 2);
// A flag, not an environment variable: `VAR=1 node ...` is POSIX shell syntax
// and `cmd` on the Windows runner answers "'VAR' is not recognized as an
// internal or external command", which is how the 0.2.0 tag failed its test job.
const strictAt = argv.indexOf('--strict');
const strict = strictAt !== -1;
if (strict) argv.splice(strictAt, 1);
const [templateArg, outArg] = argv;
if (!templateArg) {
  console.error('usage: node demo/render.mjs <template.svg> [out.png] [--lang xx] [--strict]');
  process.exit(2);
}
const template = resolve(here, templateArg);
const out = outArg ? resolve(outArg) : template.replace(/\.svg$/, '.png');

mod.setLogLevel('warn');

const fonts = new mod.FontDatabase();
const fontFiles = [join(here, 'DejaVuSans.ttf'),
  ...(existsSync(join(here, 'fonts')) ? readdirSync(join(here, 'fonts'))
    .filter((f) => /\.(ttf|otf)$/i.test(f)).map((f) => join(here, 'fonts', f)) : [])]
  .filter(existsSync);
for (const file of fontFiles) fonts.loadFontData(readFileSync(file));
// Local files first, so the generic families below point at a known face; then
// the system, for the glyphs the local files do not have (emoji, scripts).
fonts.loadSystemFonts();
// The examples ask for `sans-serif`; point every generic family at what we have.
const family = fonts.faces()[0].families[0];
for (const set of ['setSansSerifFamily', 'setSerifFamily', 'setMonospaceFamily',
                   'setCursiveFamily', 'setFantasyFamily']) fonts[set](family);
const options = { fontFamily: family, languages };

const escape = (s) => String(s)
  .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** Width of a string, straight from the renderer that will draw it. */
const measure = (text, { fontSize, family: want }) => {
  const probe = '<svg xmlns="http://www.w3.org/2000/svg">'
    + `<text id="m" font-family="${want ?? family}" font-size="${fontSize}">${escape(text)}</text></svg>`;
  return new mod.Resvg(probe, options, fonts).node('m')?.extent()?.width ?? 0;
};

const liquid = new Liquid({ root: dirname(template), extname: '', relativeReference: false });
registerSvgFilters(liquid, { measure, qrSvg });

const source = readFileSync(template, 'utf8');
const varsFile = template.replace(/\.svg$/, '.json');
const scope = existsSync(varsFile) ? JSON.parse(readFileSync(varsFile, 'utf8')) : {};

const wanted = [...referencedPartials(liquid, source)];
for (const name of wanted) {
  if (!existsSync(join(dirname(template), name))) {
    console.error(`missing partial: ${name}`);
    process.exit(1);
  }
}

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

const doc = new mod.Resvg(rendered, options, fonts);
writeFileSync(out, doc.renderPng({ scale: 2 }));

console.log(`${basename(template)}: ${doc.width}×${doc.height} units`
  + `${wanted.length ? `, partials [${wanted.join(', ')}]` : ''}`
  + `${Object.keys(scope).length ? `, vars [${Object.keys(scope).join(', ')}]` : ''} → ${out}`);
const logs = mod.takeLogs();
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
  const pendingFonts = doc.pendingFonts();
  const pendingImages = doc.pendingImages();
  const why = [
    logs.length && `${logs.length} diagnostic(s) from usvg`,
    pendingFonts.length && `unresolved font(s): ${pendingFonts.join(', ')}`,
    pendingImages.length && `unresolved image(s): ${pendingImages.join(', ')}`,
  ].filter(Boolean);
  if (why.length) {
    console.error(`  FAIL ${basename(template)}: ${why.join('; ')}`);
    process.exit(1);
  }
}
