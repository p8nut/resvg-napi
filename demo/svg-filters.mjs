// Liquid filters that only make sense when a renderer is in the room: they ask
// resvg how wide a string is, and they emit SVG rather than text.
//
// `measure(text, { fontSize, family })` returns the width the string occupies in
// user units. The page supplies it, because it owns the font database.

const escapeXml = (s) => String(s)
  .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
  .replace(/"/g, '&quot;');

const round = (n) => Math.round(n * 100) / 100;

/**
 * Registers the filters on a Liquid instance.
 *
 * @param liquid   the engine
 * @param measure  (text, { fontSize, family }) => width in user units
 * @param qrSvg    the QR generator, kept here so the whole vocabulary is in one place
 */
export function registerSvgFilters(liquid, { measure, qrSvg }) {
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
    // is a render, so keep the number of probes logarithmic.
    let lo = 0;
    let hi = text.length;
    while (lo < hi) {
      const mid = Math.ceil((lo + hi) / 2);
      if (measure(`${text.slice(0, mid).trimEnd()}…`, opts) <= width) lo = mid;
      else hi = mid - 1;
    }
    return lo > 0 ? `${text.slice(0, lo).trimEnd()}…` : '…';
  });

  /**
   * `{{ blurb | wrap: 120, 11 }}` — greedy word wrap into `<tspan>` lines. SVG
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
 * A filesystem over a Map, so `{% render "badge.svg" %}` works with fragments
 * the page holds in memory rather than files on disk.
 */
export function memoryFs(partials) {
  const read = (file) => {
    const key = String(file).replace(/^\.?\//, '');
    if (!partials.has(key)) throw new Error(`partial not loaded: ${key}`);
    return partials.get(key);
  };
  const has = (file) => partials.has(String(file).replace(/^\.?\//, ''));
  return {
    sep: '/',
    resolve: (_dir, file) => file,
    dirname: (file) => file,
    readFileSync: read,
    existsSync: has,
    readFile: async (file) => read(file),
    exists: async (file) => has(file),
  };
}

/** Partial names a template asks for, from the `render`/`include` tags. */
export function referencedPartials(liquid, source) {
  const names = new Set();
  let templates;
  try {
    templates = liquid.parse(source);
  } catch {
    return names;
  }
  const visit = (nodes, depth = 0) => {
    if (!Array.isArray(nodes) || depth > 6) return;
    for (const node of nodes) {
      if (node?.name === 'render' || node?.name === 'include') {
        // A literal file name arrives already unquoted; a dynamic one
        // (`{% render page %}`) is a Value, and only known at render time.
        const name = typeof node.file === 'string'
          ? node.file
          : String(node.file?.getText?.() ?? '').trim();
        if (name) names.add(name);
      }
      visit(node?.templates, depth + 1);
      visit(node?.elseTemplates, depth + 1);
      for (const branch of node?.branches ?? []) visit(branch?.templates, depth + 1);
    }
  };
  visit(templates);
  return names;
}
