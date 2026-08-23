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
import { Liquid } from 'liquidjs';
import { registerSvgFilters, referencedPartials } from './svg-filters.mjs';
import { qrSvg } from './liquid-entry.mjs';

const here = dirname(new URL(import.meta.url).pathname);
const require = createRequire(join(here, '..', 'index.js'));
const mod = require(join(here, '..', 'index.js'));

const [templateArg, outArg] = process.argv.slice(2);
if (!templateArg) {
  console.error('usage: node demo/render.mjs <template.svg> [out.png]');
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
if (!fonts.len()) fonts.loadSystemFonts();
// The examples ask for `sans-serif`; point every generic family at what we have.
const family = fonts.faces()[0].families[0];
for (const set of ['setSansSerifFamily', 'setSerifFamily', 'setMonospaceFamily',
                   'setCursiveFamily', 'setFantasyFamily']) fonts[set](family);
const options = { fontFamily: family };

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
const left = rendered.match(/\{[{%]/g);
if (left) {
  console.error(`${left.length} unrendered Liquid tag(s) left in the output`);
  process.exit(1);
}

const doc = new mod.Resvg(rendered, options, fonts);
writeFileSync(out, doc.renderPng({ scale: 2 }));

console.log(`${basename(template)}: ${doc.width}×${doc.height} units`
  + `${wanted.length ? `, partials [${wanted.join(', ')}]` : ''}`
  + `${Object.keys(scope).length ? `, vars [${Object.keys(scope).join(', ')}]` : ''} → ${out}`);
for (const line of mod.takeLogs()) console.log('  ' + line);
