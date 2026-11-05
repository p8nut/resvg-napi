// Liquid filters that only make sense when a renderer is in the room: they ask
// resvg how wide a string is, and they emit SVG rather than text.
//
// `measure(text, { fontSize, family })` returns the width the string occupies in
// user units. The page supplies it, because it owns the font database.

import { toValueSync } from 'liquidjs';
import qrcode from 'qrcode-generator';
import { createHelpers } from '../helpers.browser.mjs';

const escapeXml = (s) => String(s)
  .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;');

const round = (n) => Math.round(n * 100) / 100;

/**
 * Width of a string, straight from the renderer that will draw it: the
 * `measure` both pages hand to registerSvgFilters.
 *
 * The helpers' `measureText` (`resvg-napi/helpers`), on the binding the page
 * loaded, with the database it renders with: it parses with a fresh
 * `guard.ImageHrefResolver` unless `options` brings one. A string that does
 * not parse measures 0, so a template keeps rendering.
 *
 * @param binding the binding (native or wasm)
 * @param db      the `fontdb.Database` holding the fonts the document renders with
 * @param text    the string
 * @param fontSize, family  defaults: 12, `options.fontFamily`
 * @param options extra `usvg.Options` (e.g. `languages`, `fontFamily`)
 */
export function measureText(binding, db, text, { fontSize = 12, family } = {}, options = {}) {
  try {
    return createHelpers(binding).measureText(text, { ...options, fontSize, family, fonts: db });
  } catch {
    return 0;
  }
}

/**
 * QR code as an SVG fragment, sized to whatever viewport it is dropped into.
 *
 * Returns a nested `<svg>` with a viewBox and `width/height="100%"`, so
 * `<svg id="qr" x=".." y=".." width="39.68" height="39.68">{{ upn | qr | raw }}</svg>`
 * scales it without the template having to know the module count.
 *
 * @param text  value to encode
 * @param dark  colour of the modules (default black)
 * @param light colour behind them; `#00000000` for transparent (default none)
 * @param level error correction: L, M, Q or H (default M)
 */
export function qrSvg(text, dark = '#000000', light = 'none', level = 'M') {
  const value = String(text ?? '');
  if (!value) return '';

  // typeNumber 0 = smallest version that fits the data
  const qr = qrcode(0, level);
  qr.addData(value);
  qr.make();

  const n = qr.getModuleCount();
  // One path for every dark module: a single `d` beats thousands of <rect>s,
  // and crispEdges keeps the modules square at any scale.
  let d = '';
  for (let row = 0; row < n; row++) {
    for (let col = 0; col < n; col++) {
      if (qr.isDark(row, col)) d += `M${col} ${row}h1v1h-1z`;
    }
  }

  const background = light && light !== 'none'
    ? `<rect width="${n}" height="${n}" fill="${light}"/>`
    : '';
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n} ${n}"`
    + ` width="100%" height="100%" shape-rendering="crispEdges">`
    + `${background}<path d="${d}" fill="${dark}"/></svg>`;
}

/**
 * Registers the filters on a Liquid instance.
 *
 * @param liquid   the engine
 * @param measure  (text, { fontSize, family }) => width in user units
 */
export function registerSvgFilters(liquid, { measure }) {
  liquid.registerFilter('qr', (value, dark, light, level) => qrSvg(value, dark, light, level));

  /**
   * `{{ name | fit: 90, 12 }}` — cuts the string until it fits 90 user units at
   * font-size 12, ending in an ellipsis. SVG has no text overflow, so the only
   * way to know is to measure.
   */
  liquid.registerFilter('fit', (value, width, fontSize = 12, family = undefined) => {
    const text = String(value ?? '');
    if (!text || !(width > 0)) return text;
    const opts = { fontSize: Number(fontSize) || 12, family };
    if (measure(text, opts) <= width) return text;

    // Longest prefix that still fits with the ellipsis, by bisection: measuring
    // is a render, so keep the number of probes logarithmic. Cut on code
    // points: a UTF-16 index can land inside an emoji and leave half of it.
    const chars = Array.from(text);
    const prefix = (n) => chars.slice(0, n).join('').trimEnd();
    let lo = 0;
    let hi = chars.length;
    while (lo < hi) {
      const mid = Math.ceil((lo + hi) / 2);
      if (measure(`${prefix(mid)}…`, opts) <= width) lo = mid;
      else hi = mid - 1;
    }
    return lo > 0 ? `${prefix(lo)}…` : '…';
  });

  /**
   * `{{ blurb | wrap: 120, 11 | raw }}` — greedy word wrap into `<tspan>` lines. SVG
   * 1.1 does not wrap text at all, so a template otherwise has to hard-code the
   * breaks.
   */
  liquid.registerFilter('wrap', (value, width, fontSize = 12, lineHeight = undefined, family = undefined) => {
    const text = String(value ?? '').trim();
    if (!text) return '';
    const size = Number(fontSize) || 12;
    const step = Number(lineHeight) || size * 1.25;
    const opts = { fontSize: size, family };

    const lines = [];
    let line = '';
    for (const word of text.split(/\s+/)) {
      const candidate = line ? `${line} ${word}` : word;
      if (line && width > 0 && measure(candidate, opts) > width) {
        lines.push(line);
        line = word;
      } else {
        line = candidate;
      }
    }
    if (line) lines.push(line);

    return lines
      .map((l, i) => `<tspan x="0" dy="${i === 0 ? 0 : round(step)}">${escapeXml(l)}</tspan>`)
      .join('');
  });

  /**
   * `{{ scores | sparkline: 60, 16 }}` — an array of numbers becomes the
   * `points` attribute of a polyline, scaled to the box given.
   */
  liquid.registerFilter('sparkline', (value, width = 60, height = 16) => {
    const nums = (Array.isArray(value) ? value : [])
      .map(Number)
      .filter((n) => Number.isFinite(n));
    if (nums.length < 2) return '';
    const w = Number(width) || 60;
    const h = Number(height) || 16;
    const min = Math.min(...nums);
    const max = Math.max(...nums);
    const span = max - min || 1;
    return nums
      .map((n, i) => `${round((i / (nums.length - 1)) * w)},${round(h - ((n - min) / span) * h)}`)
      .join(' ');
  });

  /** `{{ text | measure: 12 }}` — the width itself, for a template doing its own layout. */
  liquid.registerFilter('measure', (value, fontSize = 12, family = undefined) =>
    round(measure(String(value ?? ''), { fontSize: Number(fontSize) || 12, family })));
}

/**
 * Calls `fn` on every node of a parsed template, nested ones included.
 * `source` is template text or what `liquid.parse` returned; text that does
 * not parse visits nothing.
 */
export function eachNode(liquid, source, fn) {
  let templates = source;
  if (typeof source === 'string') {
    try {
      templates = liquid.parse(source);
    } catch {
      return;
    }
  }
  const visit = (nodes) => {
    for (const node of nodes) {
      fn(node);
      // What liquid's own `analyze` walks: every tag lists its nested
      // templates, branches included, without our knowing their fields.
      if (node.children) visit(toValueSync(node.children(false, true)));
    }
  };
  visit(templates);
}

/** Partial names a template asks for, from the `render`/`include` tags. */
export function referencedPartials(liquid, source) {
  const names = new Set();
  eachNode(liquid, source, (node) => {
    if (node.name !== 'render' && node.name !== 'include') return;
    // A literal file name arrives already unquoted; a dynamic one
    // (`{% render page %}`) is a Value, and only known at render time.
    const name = typeof node.file === 'string'
      ? node.file
      : String(node.file?.getText?.() ?? '').trim();
    if (name) names.add(name);
  });
  return names;
}
