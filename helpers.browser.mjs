// The helpers, without a binding: `createHelpers(binding)` builds them on the
// binding it is given (ADR 0001, "Helpers sit beside it, in JS"; ADR 0002,
// Q32). `helpers.mjs` is the Node entry, bound to `./index.js`; this file is
// what a bundler's `browser` condition gets, where the binding is the loaded
// wasm module the page passes in. Nothing here loads a binding.
//
// Plain JS on the generated API only: `usvg`, `fontdb`, `tiny_skia`, `resvg`,
// `svgtypes` and `guard`. Every parse goes through a fresh
// `guard.ImageHrefResolver` and `guard.FontResolver`, so the helpers are safe
// on untrusted SVG by default, which the raw API is not.

/** The canvas cap, in pixels: an 8192x8192 canvas, 256 MiB (ADR 0002, Q25). */
export const MAX_PIXELS = 2 ** 26;

const SVGZ = 'SVGZ is not supported: inflate it first, e.g. zlib.gunzipSync(data, { maxOutputLength })';

/** An `Error` carrying a `code`, as the binding's own errors do. */
function coded(message, code) {
  const e = new Error(message);
  e.code = code;
  return e;
}

/** usvg refuses SVGZ with its own wording; #167 asked for advice instead. */
function friendlier(e) {
  if (e?.code === 'SvgzFeatureNotEnabled') return coded(SVGZ, e.code);
  return e;
}

/** gzip magic: what usvg would try to inflate, with no size limit. */
const gzipped = (data) => typeof data !== 'string' && data[0] === 0x1f && data[1] === 0x8b;

const isSource = (v) => typeof v === 'string' || v instanceof Uint8Array;

/** Keys the helpers read themselves; everything else is `usvg.Options`. */
const HELPER_KEYS = ['fonts', 'images', 'resourcesRoot', 'signal',
  'width', 'height', 'scale', 'background', 'crop', 'maxPixels'];

function usvgOptions(options) {
  const out = { ...options };
  for (const k of HELPER_KEYS) delete out[k];
  return out;
}

// --- Numbers, as Rust computes them ------------------------------------------
//
// The sizes are what the 0.9 `Resvg.renderPng` computed in Rust, byte for
// byte, so the arithmetic is Rust's: the same f32 roundings (`Math.fround`; a
// single f64 operation on f32 operands, rounded to f32, is the f32
// operation), the same saturating `as u32`, the same messages.

const f32 = Math.fround;

/** Rust's `v as u32`: truncating, saturating, NaN to 0. */
const asU32 = (v) => (v >= 4294967295 ? 4294967295 : v > 0 ? Math.trunc(v) : 0);

/** `(v as u32).max(1)`. */
const toPx = (v) => Math.max(asU32(v), 1);

/** A float as Rust's `Display` writes it (`NaN`, `inf`, no `.0`). */
function show(v, single = false) {
  if (Number.isNaN(v)) return 'NaN';
  if (v === Infinity) return 'inf';
  if (v === -Infinity) return '-inf';
  if (Object.is(v, -0)) return '-0';
  if (!single) return String(v);
  // The shortest decimal that reads back as the same f32.
  for (let p = 1; p < 10; p++) {
    const s = String(Number(v.toPrecision(p)));
    if (f32(Number(s)) === v) return s;
  }
  return String(v);
}

/** `width` / `height`: rounded, and a pixel count or an error. */
function pixels(v, name) {
  if (v === undefined || v === null) return null;
  const r = Math.sign(v) * Math.round(Math.abs(v)); // half away from zero
  if (r >= 1 && r <= 4294967295) return toPx(r);
  throw new Error(`invalid ${name}: ${show(v)}`);
}

/**
 * The size and scale of one render pass, checked against `maxPixels` before
 * anything is allocated: `{ scale, width, height }`.
 */
function plan(baseWidth, baseHeight, params = {}) {
  const width = pixels(params.width, 'width');
  const height = pixels(params.height, 'height');
  let scale; let w; let h;
  if (width !== null) {
    scale = f32(f32(width) / baseWidth);
    w = width;
    h = toPx(Math.ceil((baseHeight * width) / baseWidth));
  } else if (height !== null) {
    scale = f32(f32(height) / baseHeight);
    w = toPx(Math.ceil((baseWidth * height) / baseHeight));
    h = height;
  } else {
    const s = params.scale ?? 1;
    scale = f32(s);
    w = toPx(Math.ceil(baseWidth * s));
    h = toPx(Math.ceil(baseHeight * s));
  }
  if (!(Number.isFinite(scale) && scale > 0)) throw new Error(`invalid scale: ${show(scale, true)}`);
  const max = params.maxPixels ?? MAX_PIXELS;
  if (!(Number.isFinite(max) && max > 0)) throw new Error(`invalid maxPixels: ${show(max)}`);
  if (w * h > max) throw new Error(`${w}x${h} exceeds maxPixels (${show(max)})`);
  return { scale, width: w, height: h };
}

/**
 * A crop box as `[x, y, width, height]` in f32, from a plain
 * `{ x, y, width, height }` (what `nodeExtent` returns) or a `tiny_skia.Rect`
 * or `tiny_skia.NonZeroRect`, whose sides are methods.
 */
function cropBox(c) {
  const get = (k) => f32(typeof c[k] === 'function' ? c[k]() : c[k]);
  const [x, y, w, h] = [get('x'), get('y'), get('width'), get('height')];
  if (!(w > 0 && h > 0)) throw new Error(`empty crop: ${show(w, true)}x${show(h, true)}`);
  if (![x, y, w, h].every(Number.isFinite)) {
    throw new Error(`invalid crop: ${show(x, true)},${show(y, true)} ${show(w, true)}x${show(h, true)}`);
  }
  return [x, y, w, h];
}

// --- Fitting text to a width ------------------------------------------------
//
// Horizontal fitting for text that must not exceed a width, the way an
// Illustrator template expresses it: `data-maxwidth` on the element itself.
//
// resvg drops attributes it does not know, so the constraint can only be read
// from the source. The measurement, on the other hand, comes from the parse:
// the extent of `tree.nodeById(id)` is the width the text actually occupies
// with the fonts in hand -- which is the only number that matters.
//
// The correction is a horizontal scale appended to the element's own transform,
// matching what these templates already do (`scale(1.11 1)`). A pure geometric
// scale is exactly linear in width, so one pass lands on the target; the second
// measurement is a check, not an iteration.

const ATTR = 'data-maxwidth';

/** A `tiny_skia.NonZeroRect` as a plain `{ x, y, width, height }`, or null. */
const box = (r) => r && { x: r.x(), y: r.y(), width: r.width(), height: r.height() };

/**
 * Where a `usvg.Node` will draw, contours and filters included: a group's
 * layer box, a path's or a text's stroke box (its geometric box would clip the
 * stroke), an image's box. `null` when it has no visible size. A plain
 * `{ x, y, width, height }`, the numbers of Rust's `NonZeroRect`, which `crop`
 * takes back.
 */
export function nodeExtent({ kind, value }) {
  switch (kind) {
    case 'group': return box(value.absLayerBoundingBox());
    case 'image': return box(value.absBoundingBox().toNonZeroRect());
    default: return box(value.absStrokeBoundingBox().toNonZeroRect());
  }
}

/** `usvg::Node::abs_layer_bounding_box`: the box `resvg::render_node` offsets a node by. */
function layerBox({ kind, value }) {
  return box(kind === 'group' ? value.absLayerBoundingBox() : value.absBoundingBox().toNonZeroRect());
}

/**
 * Ratio between the tree's canvas units and the document's own coordinates.
 *
 * `width="85.6mm" viewBox="0 0 240.94 …"` makes usvg normalise the tree to the
 * physical size (323.53 units at 96 dpi), so an extent comes back 1.343× larger
 * than the numbers written in the file. A limit expressed in the document's own
 * units has to be compared in those units, or everything wider than
 * `limit / ratio` gets compressed while it actually fits.
 */
function unitScale(svg, canvasWidth) {
  const viewBox = svg.match(/\sviewBox\s*=\s*["']\s*[-\d.]+[\s,]+[-\d.]+[\s,]+([\d.]+)/);
  const width = viewBox ? Number(viewBox[1]) : NaN;
  if (!Number.isFinite(width) || width <= 0) return 1;
  return canvasWidth / width;
}

/**
 * An attribute's value from a tag's attribute text, either quote style. The
 * name must start the text or follow whitespace: `\bid` also matched the `id`
 * of `data-id`.
 */
function attr(attrs, name) {
  return attrs.match(new RegExp(`(?:^|\\s)${name}\\s*=\\s*(["'])(.*?)\\1`))?.[2];
}

/** Elements carrying a width constraint, with the tag text needed to rewrite it. */
export function findWidthConstraints(svg) {
  const found = [];
  for (const match of svg.matchAll(/<([a-zA-Z][\w:.-]*)\b([^>]*?)(\/?)>/g)) {
    const [tag, name, attrs] = match;
    const max = attr(attrs, ATTR);
    if (max === undefined) continue;
    const width = Number(max);
    const id = attr(attrs, 'id');
    found.push({ tag, name, attrs, id, max: width, index: match.index });
  }
  return found;
}

/**
 * Appends a horizontal scale to a tag's transform, creating one if absent.
 *
 * Anchored at the element's own `x`: a bare `scale()` is anchored at the origin,
 * which slides the text left as it narrows -- 30.72 to 3.98 on a `x="28"` text.
 * `translate(x) scale(k) translate(-x)` compresses it in place, and collapses to
 * a plain scale when x is 0, which is the case for anything positioned by a
 * transform.
 */
function withHorizontalScale(tag, attrs, factor) {
  // `x` may be a list for per-glyph positioning; the first value is the anchor.
  const anchor = Number.parseFloat(attr(attrs, 'x')?.trim() ?? '0') || 0;
  const scale = anchor
    ? `translate(${round(anchor)} 0) scale(${round(factor)} 1) translate(${round(-anchor)} 0)`
    : `scale(${round(factor)} 1)`;
  const existing = attrs.match(/(?:^|\s)transform\s*=\s*(["'])(.*?)\1/);
  if (existing) {
    // Composed on the right: the element keeps its position, the glyphs
    // compress toward the anchor the transform established. Same quotes back,
    // so a `'` value cannot end up holding a `"` or the other way round.
    const q = existing[1];
    return tag.replace(existing[0], `${existing[0].match(/^\s*/)[0]}transform=${q}${existing[2].trim()} ${scale}${q}`);
  }
  return tag.replace(/(\/?)>$/, ` transform="${scale}"$1>`);
}

const round = (n) => Math.round(n * 10000) / 10000;

/**
 * Measuring goes through `nodeById(id)`, but a template has no reason to carry ids
 * just to be measurable. Elements that lack one get a generated id, checked
 * against the source so it cannot collide.
 *
 * Insertions run backwards so the offsets still ahead of them stay valid. The
 * caller then re-reads the constraints from the rewritten source: patching the
 * old offsets by hand is how the first version corrupted the document.
 */
function ensureIds(svg) {
  const missing = findWidthConstraints(svg).filter((c) => !c.id);
  if (!missing.length) return svg;

  // Numbered in document order, so a diagnostic about `fit-1` refers to the
  // first constrained element, then spliced back to front.
  let n = 0;
  const named = missing.map((c) => {
    let id;
    do {
      id = `fit-${++n}`;
    } while (svg.includes(`"${id}"`));
    return { ...c, id };
  });

  let out = svg;
  for (const c of named.reverse()) {
    const tagged = c.tag.replace(/^<([a-zA-Z][\w:.-]*)/, `<$1 id="${c.id}"`);
    out = out.slice(0, c.index) + tagged + out.slice(c.index + c.tag.length);
  }
  return out;
}

/**
 * A `render` for `fitTextWidths` that is safe on untrusted SVG: each parse
 * gets a fresh `guard.ImageHrefResolver` (images supplied by href, files only
 * under `resourcesDir`, SVGZ and oversized images refused). The raw
 * `usvg.Tree.fromStr` resolves `<image href>` from any local path instead.
 *
 * @param binding  the binding (native or wasm): `{ usvg, guard }`
 * @param options  `usvg.Options` (`fontdb`, `fontFamily`, `resourcesDir`...),
 *                 plus the resolver's `images` and `resourcesRoot`
 * @returns (svg) => usvg.Tree
 */
export function guardedParse({ usvg, guard }, { images, resourcesRoot, ...options } = {}) {
  return (svg) => usvg.Tree.fromStr(svg, {
    ...options,
    imageHrefResolver: new guard.ImageHrefResolver({ images, resourcesRoot }),
  });
}

/**
 * Fits every `data-maxwidth` element by compressing it horizontally.
 *
 * @param svg     source, after any templating
 * @param render  (svg) => usvg.Tree; supply fonts and options here.
 *                 `guardedParse(binding, options)` is the safe default. The raw
 *                 usvg.Tree.fromStr resolves <image href> from any local path:
 *                 do not pass untrusted SVG through it without a
 *                 `guard.ImageHrefResolver`.
 * @returns { svg, adjustments, problems }
 */
export function fitTextWidths(svg, render) {
  const problems = [];
  if (!findWidthConstraints(svg).length) return { svg, adjustments: [], problems };

  // Every constrained element carries an id from here on, and the offsets are
  // read from this exact string.
  svg = ensureIds(svg);
  const constraints = findWidthConstraints(svg);

  let tree;
  try {
    tree = render(svg);
  } catch (e) {
    return { svg, adjustments: [], problems: [`could not measure: ${e.message}`] };
  }

  // Every width below is in the document's own units, like the attribute.
  const scale = unitScale(svg, tree.size().width());
  let out = svg;
  const adjustments = [];
  const splices = [];
  for (const c of constraints) {
    if (!Number.isFinite(c.max) || c.max <= 0) {
      problems.push(`#${c.id}: ${ATTR}="${c.max}" is not a usable width`);
      continue;
    }
    const node = tree.nodeById(c.id);
    const extent = node && nodeExtent(node);
    if (!extent) {
      problems.push(`#${c.id} has no visible extent: nothing to fit`);
      continue;
    }
    const width = extent.width / scale;
    if (width <= c.max) continue;

    const factor = c.max / width;
    splices.push({ index: c.index, length: c.tag.length,
                   text: withHorizontalScale(c.tag, c.attrs, factor) });
    adjustments.push({ id: c.id, from: round(width), to: c.max, factor: round(factor) });
  }

  // Backwards, so each offset still points where it did when it was taken.
  for (const s of splices.sort((a, b) => b.index - a.index)) {
    out = out.slice(0, s.index) + s.text + out.slice(s.index + s.length);
  }

  if (!adjustments.length) return { svg: out, adjustments, problems };

  // Verify rather than trust: report what the widths actually became.
  try {
    const after = render(out);
    const afterScale = unitScale(out, after.size().width());
    for (const a of adjustments) {
      const node = after.nodeById(a.id);
      const got = node ? nodeExtent(node)?.width : undefined;
      a.measured = got === undefined ? NaN : round(got / afterScale);
    }
  } catch (e) {
    problems.push(`could not verify: ${e.message}`);
  }
  return { svg: out, adjustments, problems };
}

// --- The helper set ----------------------------------------------------------

const sets = new WeakMap();

/**
 * The helpers, built on `binding`: the native module in Node (`helpers.mjs`
 * passes `./index.js`), the loaded wasm module in a browser. One set per
 * binding, so the system fonts are scanned once per binding whoever asks.
 */
export function createHelpers(binding) {
  if (!binding?.usvg || !binding.guard || !binding.resvg || !binding.tiny_skia
      || !binding.fontdb || !binding.svgtypes) {
    throw new TypeError('createHelpers(binding): pass the loaded resvg-napi module');
  }
  let set = sets.get(binding);
  if (!set) {
    set = build(binding);
    sets.set(binding, set);
  }
  return set;
}

function build(binding) {
  const { usvg, fontdb, tiny_skia, resvg, svgtypes, guard } = binding;

  // System fonts: scanned once, on first use, and shared by every parse that
  // brings no fonts of its own. The async path scans on a worker thread; the
  // sync path scans in place, and whichever finishes first is kept.
  let system = null;
  let pending = null;

  function systemFonts() {
    if (!system) {
      const db = new fontdb.Database();
      db.loadSystemFonts();
      system = db;
    }
    return system;
  }

  function systemFontsAsync(signal) {
    if (system) return Promise.resolve(system);
    pending ??= (async () => {
      const db = new fontdb.Database();
      try {
        await db.loadSystemFontsAsync();
      } catch (e) {
        pending = null;
        throw e;
      }
      if (system) {
        db.dispose();
      } else {
        system = db;
      }
      return system;
    })();
    if (!signal) return pending;
    // The scan is shared, so a signal abandons the wait, not the scan.
    if (signal.aborted) return Promise.reject(signal.reason);
    return new Promise((resolve, reject) => {
      const abort = () => reject(signal.reason);
      signal.addEventListener('abort', abort, { once: true });
      pending.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort));
    });
  }

  /** `fonts` as a `fontdb.Database`: a database, or files and buffers to load. */
  function database(fonts) {
    if (fonts instanceof fontdb.Database) return fonts;
    if (!Array.isArray(fonts)) {
      throw new TypeError('fonts: a fontdb.Database, or an array of font paths and buffers');
    }
    const db = new fontdb.Database();
    for (const f of fonts) {
      if (typeof f === 'string') db.loadFontFile(f);
      else db.loadFontData(f);
    }
    return db;
  }

  /** Refuses what is not a document, and SVGZ, before anything is loaded. */
  function check(svg) {
    if (!isSource(svg)) throw new TypeError('svg: a string or a Uint8Array');
    if (gzipped(svg)) throw coded(SVGZ, 'SvgzFeatureNotEnabled');
  }

  /** The parse's options and the providers it reads its misses from. */
  function prepare(options, fonts) {
    const images = options.imageHrefResolver
      ?? new guard.ImageHrefResolver({ images: options.images, resourcesRoot: options.resourcesRoot });
    const families = options.fontResolver ?? new guard.FontResolver();
    const opts = { ...usvgOptions(options), fontdb: fonts, imageHrefResolver: images, fontResolver: families };
    const owned = [options.imageHrefResolver ? null : images, options.fontResolver ? null : families];
    return { opts, images, families, owned };
  }

  function parsed(tree, { images, families, owned }) {
    const out = { tree, pendingImages: images.misses(), pendingFonts: families.misses() };
    for (const p of owned) p?.dispose();
    return out;
  }

  /** The fonts a parse asked for; `fontdb`, usvg's name, is read too. */
  const given = (options) => options.fonts ?? options.fontdb;

  function parse(svg, options) {
    options ??= {};
    check(svg);
    const fonts = given(options) == null ? systemFonts() : database(given(options));
    const p = prepare(options, fonts);
    let tree;
    try {
      tree = typeof svg === 'string' ? usvg.Tree.fromStr(svg, p.opts) : usvg.Tree.fromData(svg, p.opts);
    } catch (e) {
      throw friendlier(e);
    }
    return parsed(tree, p);
  }

  async function parseAsync(svg, options) {
    options ??= {};
    const { signal } = options;
    check(svg);
    const fonts = given(options) == null
      ? await systemFontsAsync(signal) : database(given(options));
    const p = prepare(options, fonts);
    let tree;
    try {
      tree = await (typeof svg === 'string'
        ? usvg.Tree.fromStrAsync(svg, p.opts, signal)
        : usvg.Tree.fromDataAsync(svg, p.opts, signal));
    } catch (e) {
      throw friendlier(e);
    }
    return parsed(tree, p);
  }

  /** A blank pixmap of the planned size, filled with `background`. */
  function pixmapFor(size, background) {
    let color = null;
    if (background !== undefined && background !== null) {
      let c;
      try {
        c = svgtypes.Color.fromStr(background);
      } catch {
        throw new Error(`invalid background: ${background}`);
      }
      color = tiny_skia.Color.fromRgba8(c.red, c.green, c.blue, c.alpha);
    }
    let pixmap;
    try {
      pixmap = new tiny_skia.Pixmap(size.width, size.height);
    } catch {
      throw new Error(`bad pixmap size ${size.width}x${size.height}`);
    }
    if (color) pixmap.fill(color);
    return pixmap;
  }

  /** The canvas and root transform of a whole-tree render (`crop` included). */
  function treeCanvas(tree, params) {
    let x = 0; let y = 0; let w; let h;
    if (params.crop) {
      [x, y, w, h] = cropBox(params.crop);
    } else {
      const size = tree.size();
      [w, h] = [size.width(), size.height()];
    }
    const size = plan(w, h, params);
    const transform = tiny_skia.Transform.fromScale(size.scale, size.scale).preTranslate(-x, -y);
    return { pixmap: pixmapFor(size, params.background), transform };
  }

  /**
   * The canvas and root transform of a node render, sized to the node's
   * extent. `resvg.renderNode` offsets the node by its layer box and applies a
   * group's own transform itself, so both are undone here: a group is passed
   * its parent's transform. (The 0.9 `renderNodePng` passed the group's
   * absolute one, which applied its own transform twice.)
   */
  function nodeCanvas(node, params) {
    if (params.crop) throw coded('crop is not supported when rendering a single node', 'InvalidArg');
    const box = nodeExtent(node);
    if (!box) throw new Error('element has no visible size');
    const inner = layerBox(node);
    if (!inner) throw new Error('element is empty');
    let placed = node.value.absTransform();
    if (node.kind === 'group' && !node.value.transform().isIdentity()) {
      const own = node.value.transform().invert();
      if (!own) throw new Error('element is empty');
      placed = placed.preConcat(own);
    }
    const size = plan(box.width, box.height, params);
    const transform = tiny_skia.Transform.fromScale(size.scale, size.scale)
      .preTranslate(-box.x, -box.y)
      .preConcat(placed)
      .preTranslate(inner.x, inner.y);
    return { pixmap: pixmapFor(size, params.background), transform };
  }

  /** The tree to draw, and whether it is ours to dispose. */
  function treeOf(input, options) {
    if (!isSource(input)) return { tree: input, owned: false };
    return { tree: parse(input, options).tree, owned: true };
  }

  async function treeOfAsync(input, options) {
    if (!isSource(input)) return { tree: input, owned: false };
    return { tree: (await parseAsync(input, options)).tree, owned: true };
  }

  /** Runs `draw` on a fresh canvas, and frees what it made, whatever happens. */
  function drawn({ tree, owned }, params, draw) {
    let pixmap;
    try {
      const canvas = treeCanvas(tree, params);
      pixmap = canvas.pixmap;
      resvg.render(tree, canvas.transform, pixmap);
      return draw(pixmap);
    } finally {
      if (pixmap && !pixmap.disposed) pixmap.dispose();
      if (owned) tree.dispose();
    }
  }

  async function drawnAsync({ tree, owned }, params, draw) {
    let pixmap;
    try {
      const canvas = treeCanvas(tree, params);
      pixmap = canvas.pixmap;
      await resvg.renderAsync(tree, canvas.transform, pixmap, params.signal);
      return await draw(pixmap);
    } finally {
      if (pixmap && !pixmap.disposed) pixmap.dispose();
      if (owned) tree.dispose();
    }
  }

  const raw = (pixmap) => {
    const width = pixmap.width();
    const height = pixmap.height();
    return { width, height, data: pixmap.takeDemultiplied() };
  };

  function renderPng(input, options) {
    options ??= {};
    return drawn(treeOf(input, options), options, (p) => p.encodePng());
  }

  async function renderPngAsync(input, options) {
    options ??= {};
    return drawnAsync(await treeOfAsync(input, options), options,
      (p) => p.encodePngAsync(options.signal));
  }

  function renderRaw(input, options) {
    options ??= {};
    return drawn(treeOf(input, options), options, raw);
  }

  async function renderRawAsync(input, options) {
    options ??= {};
    return drawnAsync(await treeOfAsync(input, options), options, raw);
  }

  function renderNodePng(node, params) {
    params ??= {};
    const { pixmap, transform } = nodeCanvas(node, params);
    try {
      if (!resvg.renderNode(node, transform, pixmap)) throw new Error('element is empty');
      return pixmap.encodePng();
    } finally {
      pixmap.dispose();
    }
  }

  async function renderNodePngAsync(node, params) {
    params ??= {};
    const { pixmap, transform } = nodeCanvas(node, params);
    try {
      if (!await resvg.renderNodeAsync(node, transform, pixmap, params.signal)) {
        throw new Error('element is empty');
      }
      return await pixmap.encodePngAsync(params.signal);
    } finally {
      if (!pixmap.disposed) pixmap.dispose();
    }
  }

  const escapeXml = (s) => String(s)
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;');

  function measureText(text, { fontSize = 12, family, ...options } = {}) {
    const fam = family ?? options.fontFamily ?? 'sans-serif';
    const probe = '<svg xmlns="http://www.w3.org/2000/svg">'
      + `<text id="m" font-family="${escapeXml(fam)}" font-size="${escapeXml(fontSize)}">`
      + `${escapeXml(text)}</text></svg>`;
    const { tree } = parse(probe, { ...options, fontFamily: fam });
    try {
      const node = tree.nodeById('m');
      return (node && nodeExtent(node)?.width) ?? 0;
    } finally {
      tree.dispose();
    }
  }

  function fit(svg, render = {}) {
    if (typeof render === 'function') return fitTextWidths(svg, render);
    return fitTextWidths(svg, (s) => parse(s, render).tree);
  }

  return Object.freeze({
    parse,
    parseAsync,
    renderPng,
    renderPngAsync,
    renderRaw,
    renderRawAsync,
    renderNodePng,
    renderNodePngAsync,
    measureText,
    fitTextWidths: fit,
    systemFonts,
    systemFontsAsync,
  });
}
