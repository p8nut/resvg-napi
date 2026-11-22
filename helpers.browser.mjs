/**
 * The helpers, without a binding: `createHelpers(binding)` builds them on the
 * binding it is given (ADR 0001, "Helpers sit beside it, in JS"; ADR 0002,
 * Q32). What a bundler's `browser` condition resolves `resvg-napi/helpers`
 * to, where the binding is the loaded wasm module the page passes in; in
 * Node, `helpers.mjs` (`helpers.d.mts`) adds the set bound to the native
 * binding, `./index.js`. Nothing here loads a binding.
 *
 * Code on the generated API only: `usvg`, `fontdb`, `tiny_skia`, `resvg`,
 * `svgtypes` and `guard`. Every parse goes through a fresh
 * `guard.ImageHrefResolver` and `guard.FontResolver`, so the helpers are safe
 * on untrusted SVG by default, which the raw API is not.
 *
 * `helpers.browser.mjs` and `helpers.browser.d.mts` are emitted by `tsc`
 * (`npm run build:helpers`) from `helpers.browser.mts`: edit that one.
 * `npm run typecheck` reads the declarations against `test/helpers-types.mts`.
 */
/**
 * The canvas cap the helpers default to: 2^26 px, an 8192x8192 canvas or
 * 256 MiB (ADR 0002, Q25).
 */
export const MAX_PIXELS = 2 ** 26;
const SVGZ = 'SVGZ is not supported: inflate it first, e.g. zlib.gunzipSync(data, { maxOutputLength })';
/** An `Error` carrying a `code`, as the binding's own errors do. */
function coded(message, code) {
    return Object.assign(new Error(message), { code });
}
/** usvg refuses SVGZ with its own wording; #167 asked for advice instead. */
function friendlier(e) {
    if (typeof e === 'object' && e !== null && 'code' in e && e.code === 'SvgzFeatureNotEnabled') {
        return coded(SVGZ, e.code);
    }
    return e;
}
/** `e.message`, read off whatever was thrown. */
const messageOf = (e) => (typeof e === 'object' && e !== null && 'message' in e ? e.message : undefined);
/** gzip magic: what usvg would try to inflate, with no size limit. */
const gzipped = (data) => typeof data !== 'string' && data[0] === 0x1f && data[1] === 0x8b;
const isSource = (v) => typeof v === 'string' || v instanceof Uint8Array;
/** Keys the helpers read themselves; everything else is `usvg.Options`. */
function usvgOptions({ fonts, images, resourcesRoot, signal, width, height, scale, background, crop, maxPixels, ...options }) {
    return options;
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
    if (Number.isNaN(v))
        return 'NaN';
    if (v === Infinity)
        return 'inf';
    if (v === -Infinity)
        return '-inf';
    if (Object.is(v, -0))
        return '-0';
    if (!single)
        return String(v);
    // The shortest decimal that reads back as the same f32.
    for (let p = 1; p < 10; p++) {
        const s = String(Number(v.toPrecision(p)));
        if (f32(Number(s)) === v)
            return s;
    }
    return String(v);
}
/** `width` / `height`: rounded, and a pixel count or an error. */
function pixels(v, name) {
    if (v === undefined || v === null)
        return null;
    const r = Math.sign(v) * Math.round(Math.abs(v)); // half away from zero
    if (r >= 1 && r <= 4294967295)
        return toPx(r);
    throw new Error(`invalid ${name}: ${show(v)}`);
}
/**
 * The size and scale of one render pass, checked against `maxPixels` before
 * anything is allocated: `{ scale, width, height }`.
 */
function plan(baseWidth, baseHeight, params = {}) {
    const width = pixels(params.width, 'width');
    const height = pixels(params.height, 'height');
    let scale;
    let w;
    let h;
    if (width !== null) {
        scale = f32(f32(width) / baseWidth);
        w = width;
        h = toPx(Math.ceil((baseHeight * width) / baseWidth));
    }
    else if (height !== null) {
        scale = f32(f32(height) / baseHeight);
        w = toPx(Math.ceil((baseWidth * height) / baseHeight));
        h = height;
    }
    else {
        const s = params.scale ?? 1;
        scale = f32(s);
        w = toPx(Math.ceil(baseWidth * s));
        h = toPx(Math.ceil(baseHeight * s));
    }
    if (!(Number.isFinite(scale) && scale > 0))
        throw new Error(`invalid scale: ${show(scale, true)}`);
    const max = params.maxPixels ?? MAX_PIXELS;
    if (!(Number.isFinite(max) && max > 0))
        throw new Error(`invalid maxPixels: ${show(max)}`);
    if (w * h > max)
        throw new Error(`${w}x${h} exceeds maxPixels (${show(max)})`);
    return { scale, width: w, height: h };
}
/**
 * A crop box as `[x, y, width, height]` in f32, from a plain
 * `{ x, y, width, height }` (what `nodeExtent` returns) or a `tiny_skia.Rect`
 * or `tiny_skia.NonZeroRect`, whose sides are methods.
 */
function cropBox(c) {
    const get = (k) => {
        const side = c[k];
        return f32(typeof side === 'function' ? side.call(c) : side);
    };
    const [x, y, w, h] = [get('x'), get('y'), get('width'), get('height')];
    if (!(w > 0 && h > 0))
        throw new Error(`empty crop: ${show(w, true)}x${show(h, true)}`);
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
const box = (r) => (r ? { x: r.x(), y: r.y(), width: r.width(), height: r.height() } : null);
/**
 * Where a `usvg.Node` will draw, contours and filters included: a group's
 * layer box, a path's or a text's stroke box (its geometric box would clip the
 * stroke), an image's box. `null` when it has no visible size. A plain
 * `{ x, y, width, height }`, the numbers of Rust's `NonZeroRect`, which `crop`
 * takes back.
 */
export function nodeExtent(node) {
    const { kind, value } = node;
    switch (kind) {
        case 'group': return box(value.absLayerBoundingBox());
        case 'image': return box(value.absBoundingBox().toNonZeroRect());
        default: return box(value.absStrokeBoundingBox()?.toNonZeroRect());
    }
}
/** `usvg::Node::abs_layer_bounding_box`: the box `resvg::render_node` offsets a node by. */
function layerBox({ kind, value }) {
    return box(kind === 'group' ? value.absLayerBoundingBox() : value.absBoundingBox()?.toNonZeroRect());
}
/**
 * Ratio between the tree's canvas units and the document's own coordinates.
 *
 * `width="85.6mm" viewBox="0 0 240.94 …"` makes usvg normalise the tree to the
 * physical size (323.53 units at 96 dpi), so an extent comes back 1.343× larger
 * than the numbers written in the file. A limit expressed in the document's own
 * units has to be compared in those units, or everything wider than
 * `limit / ratio` gets compressed while it actually fits.
 *
 * The viewBox is fitted into the canvas the way `preserveAspectRatio` says:
 * `meet` (the default) by the smaller of the two axis ratios, `slice` by the
 * larger, `none` by each axis on its own, the horizontal one here.
 */
function unitScale(svg, canvas) {
    const root = masked(svg).match(/<svg\b([^>]*)>/);
    const attrs = root ? root[1] : '';
    const viewBox = (attr(attrs, 'viewBox') ?? '').trim().split(/\s*,\s*|\s+/).map(Number);
    const [, , width, height] = viewBox;
    if (viewBox.length !== 4 || !(width > 0) || !(height > 0))
        return 1;
    const sx = canvas.width() / width;
    const sy = canvas.height() / height;
    const aspect = (attr(attrs, 'preserveAspectRatio') ?? '').trim().split(/\s+/);
    if (aspect.includes('none'))
        return sx;
    return aspect.includes('slice') ? Math.max(sx, sy) : Math.min(sx, sy);
}
/**
 * `svg` with its comments and CDATA sections blanked to spaces: markup inside
 * them is not an element, and the offsets of everything else stay put.
 */
function masked(svg) {
    return svg.replace(/<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>/g, (m) => ' '.repeat(m.length));
}
/**
 * An attribute's value from a tag's attribute text, either quote style. The
 * name must start the text or follow whitespace: `\bid` also matched the `id`
 * of `data-id`.
 */
function attr(attrs, name) {
    return attrs.match(new RegExp(`(?:^|\\s)${name}\\s*=\\s*(["'])(.*?)\\1`))?.[2];
}
/**
 * Elements carrying a width constraint, with the tag text needed to rewrite it.
 * Tags inside comments and CDATA sections are not elements, so they are skipped.
 */
export function findWidthConstraints(svg) {
    const found = [];
    // Masking keeps every offset, and a tag outside a comment reads the same.
    for (const match of masked(svg).matchAll(/<([a-zA-Z][\w:.-]*)\b([^>]*?)(\/?)>/g)) {
        const [tag, name, attrs] = match;
        const max = attr(attrs, ATTR);
        if (max === undefined)
            continue;
        const width = Number(max);
        const id = attr(attrs, 'id');
        found.push({ tag, name, attrs, id: id ?? null, max: width, index: match.index });
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
        // `^\s*` matches any string, so `match` never returns null here.
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
    if (!missing.length)
        return svg;
    // Numbered in document order, so a diagnostic about `fit-1` refers to the
    // first constrained element, then spliced back to front.
    let n = 0;
    const named = missing.map((c) => {
        let id;
        // Taken in either quote style. `\b` also takes `data-id` and `xml:id`: a
        // name skipped for nothing costs less than a duplicate id.
        do {
            id = `fit-${++n}`;
        } while (new RegExp(`\\bid\\s*=\\s*(["'])${id}\\1`).test(svg));
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
export function guardedParse(binding, options = {}) {
    const { usvg, guard } = binding;
    const { images, resourcesRoot, ...parseOptions } = options;
    return (svg) => usvg.Tree.fromStr(svg, {
        ...parseOptions,
        imageHrefResolver: new guard.ImageHrefResolver({ images, resourcesRoot }),
    });
}
/**
 * Fits every `data-maxwidth` element by compressing it horizontally.
 *
 * @param svg     source, after any templating
 * @param render  `(svg) => usvg.Tree`; supply fonts and options here.
 *                 `guardedParse(binding, options)` is the safe default. The
 *                 raw `usvg.Tree.fromStr` resolves `<image href>` from any
 *                 local path: do not pass untrusted SVG through it without a
 *                 `guard.ImageHrefResolver`.
 * @returns `{ svg, adjustments, problems }`
 */
export function fitTextWidths(svg, render) {
    const problems = [];
    if (!findWidthConstraints(svg).length)
        return { svg, adjustments: [], problems };
    // Every constrained element carries an id from here on, and the offsets are
    // read from this exact string.
    svg = ensureIds(svg);
    const constraints = findWidthConstraints(svg);
    let tree;
    try {
        tree = render(svg);
    }
    catch (e) {
        return { svg, adjustments: [], problems: [`could not measure: ${messageOf(e)}`] };
    }
    // Every width below is in the document's own units, like the attribute.
    const scale = unitScale(svg, tree.size());
    let out = svg;
    const adjustments = [];
    const splices = [];
    for (const c of constraints) {
        // ensureIds gave every constrained element an id.
        const id = c.id;
        if (!Number.isFinite(c.max) || c.max <= 0) {
            problems.push(`#${id}: ${ATTR}="${c.max}" is not a usable width`);
            continue;
        }
        const node = tree.nodeById(id);
        const extent = node && nodeExtent(node);
        if (!extent) {
            problems.push(`#${id} has no visible extent: nothing to fit`);
            continue;
        }
        const width = extent.width / scale;
        if (width <= c.max)
            continue;
        const factor = c.max / width;
        splices.push({ index: c.index, length: c.tag.length,
            text: withHorizontalScale(c.tag, c.attrs, factor) });
        adjustments.push({ id, from: round(width), to: c.max, factor: round(factor) });
    }
    // Backwards, so each offset still points where it did when it was taken.
    for (const s of splices.sort((a, b) => b.index - a.index)) {
        out = out.slice(0, s.index) + s.text + out.slice(s.index + s.length);
    }
    if (!adjustments.length)
        return { svg: out, adjustments, problems };
    // Verify rather than trust: report what the widths actually became.
    try {
        const after = render(out);
        const afterScale = unitScale(out, after.size());
        for (const a of adjustments) {
            const node = after.nodeById(a.id);
            const got = node ? nodeExtent(node)?.width : undefined;
            a.measured = got === undefined ? NaN : round(got / afterScale);
        }
    }
    catch (e) {
        problems.push(`could not verify: ${messageOf(e)}`);
    }
    return { svg: out, adjustments, problems };
}
// --- The helper set ----------------------------------------------------------
const sets = new WeakMap();
/**
 * The helpers on `binding`: the native module in Node (`helpers.mjs` passes
 * `./index.js`), in a browser the wasm module the page loaded. One set per
 * binding, so the system fonts are scanned once per binding whoever asks.
 * Throws a `TypeError` when `binding` is not a loaded resvg-napi module.
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
        if (system)
            return Promise.resolve(system);
        const scan = pending ??= (async () => {
            const db = new fontdb.Database();
            try {
                await db.loadSystemFontsAsync();
            }
            catch (e) {
                pending = null;
                throw e;
            }
            if (system) {
                db.dispose();
            }
            else {
                system = db;
            }
            return system;
        })();
        if (!signal)
            return scan;
        // The scan is shared, so a signal abandons the wait, not the scan.
        if (signal.aborted)
            return Promise.reject(signal.reason);
        return new Promise((resolve, reject) => {
            const abort = () => reject(signal.reason);
            signal.addEventListener('abort', abort, { once: true });
            scan.then(resolve, reject).finally(() => signal.removeEventListener('abort', abort));
        });
    }
    /** `fonts` as a `fontdb.Database`: a database, or files and buffers to load. */
    function database(fonts) {
        if (fonts instanceof fontdb.Database)
            return fonts;
        if (!Array.isArray(fonts)) {
            throw new TypeError('fonts: a fontdb.Database, or an array of font paths and buffers');
        }
        const db = new fontdb.Database();
        try {
            for (const f of fonts) {
                if (typeof f === 'string')
                    db.loadFontFile(f);
                else
                    db.loadFontData(f);
            }
        }
        catch (e) {
            db.dispose();
            throw e;
        }
        return db;
    }
    /** Refuses what is not a document, and SVGZ, before anything is loaded. */
    function check(svg) {
        if (!isSource(svg))
            throw new TypeError('svg: a string or a Uint8Array');
        if (gzipped(svg))
            throw coded(SVGZ, 'SvgzFeatureNotEnabled');
    }
    /**
     * The parse's options and the providers it reads its misses from. `fonts`
     * is owned when the parse built it from a list: the tree holds its own
     * reference to the faces, so the database goes with the resolvers.
     */
    function prepare(options, fonts, ownsFonts) {
        const owned = ownsFonts ? [fonts] : [];
        try {
            const images = options.imageHrefResolver
                ?? new guard.ImageHrefResolver({ images: options.images, resourcesRoot: options.resourcesRoot });
            if (!options.imageHrefResolver)
                owned.push(images);
            const families = options.fontResolver ?? new guard.FontResolver();
            if (!options.fontResolver)
                owned.push(families);
            const opts = { ...usvgOptions(options), fontdb: fonts, imageHrefResolver: images, fontResolver: families };
            return { opts, images, families, owned };
        }
        catch (e) {
            release({ owned });
            throw e;
        }
    }
    /** Disposes what the parse made for itself, whether it succeeded or not. */
    function release({ owned }) {
        for (const p of owned)
            p.dispose();
    }
    const misses = (tree, { images, families }) => ({ tree, pendingImages: images.misses(), pendingFonts: families.misses() });
    /** The fonts a parse asked for; `fontdb`, usvg's name, is read too. */
    const given = (options) => options.fonts ?? options.fontdb;
    function parse(svg, options) {
        options ??= {};
        check(svg);
        const asked = given(options);
        const fonts = asked == null ? systemFonts() : database(asked);
        const p = prepare(options, fonts, asked != null && fonts !== asked);
        try {
            const tree = typeof svg === 'string' ? usvg.Tree.fromStr(svg, p.opts) : usvg.Tree.fromData(svg, p.opts);
            return misses(tree, p);
        }
        catch (e) {
            throw friendlier(e);
        }
        finally {
            release(p);
        }
    }
    async function parseAsync(svg, options) {
        options ??= {};
        const { signal } = options;
        check(svg);
        const asked = given(options);
        const fonts = asked == null ? await systemFontsAsync(signal) : database(asked);
        const p = prepare(options, fonts, asked != null && fonts !== asked);
        try {
            const tree = await (typeof svg === 'string'
                ? usvg.Tree.fromStrAsync(svg, p.opts, signal)
                : usvg.Tree.fromDataAsync(svg, p.opts, signal));
            return misses(tree, p);
        }
        catch (e) {
            throw friendlier(e);
        }
        finally {
            release(p);
        }
    }
    /** A blank pixmap of the planned size, filled with `background`. */
    function pixmapFor(size, background) {
        let color = null;
        if (background !== undefined && background !== null) {
            let c;
            try {
                c = svgtypes.Color.fromStr(background);
            }
            catch {
                throw new Error(`invalid background: ${background}`);
            }
            color = tiny_skia.Color.fromRgba8(c.red, c.green, c.blue, c.alpha);
        }
        let pixmap;
        try {
            pixmap = new tiny_skia.Pixmap(size.width, size.height);
        }
        catch {
            throw new Error(`bad pixmap size ${size.width}x${size.height}`);
        }
        if (color)
            pixmap.fill(color);
        return pixmap;
    }
    /** The canvas and root transform of a whole-tree render (`crop` included). */
    function treeCanvas(tree, params) {
        let x = 0;
        let y = 0;
        let w;
        let h;
        if (params.crop) {
            [x, y, w, h] = cropBox(params.crop);
        }
        else {
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
        if (params.crop)
            throw coded('crop is not supported when rendering a single node', 'InvalidArg');
        const box = nodeExtent(node);
        if (!box)
            throw new Error('element has no visible size');
        const inner = layerBox(node);
        if (!inner)
            throw new Error('element is empty');
        let placed = node.value.absTransform();
        if (node.kind === 'group' && !node.value.transform().isIdentity()) {
            const own = node.value.transform().invert();
            if (!own)
                throw new Error('element is empty');
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
        if (!isSource(input))
            return { tree: input, owned: false };
        return { tree: parse(input, options).tree, owned: true };
    }
    async function treeOfAsync(input, options) {
        if (!isSource(input))
            return { tree: input, owned: false };
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
        }
        finally {
            if (pixmap && !pixmap.disposed)
                pixmap.dispose();
            if (owned)
                tree.dispose();
        }
    }
    async function drawnAsync({ tree, owned }, params, draw) {
        let pixmap;
        try {
            const canvas = treeCanvas(tree, params);
            pixmap = canvas.pixmap;
            await resvg.renderAsync(tree, canvas.transform, pixmap, params.signal);
            return await draw(pixmap);
        }
        finally {
            if (pixmap && !pixmap.disposed)
                pixmap.dispose();
            if (owned)
                tree.dispose();
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
        const { signal } = options;
        return drawnAsync(await treeOfAsync(input, options), options, (p) => p.encodePngAsync(signal));
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
            if (!resvg.renderNode(node, transform, pixmap))
                throw new Error('element is empty');
            return pixmap.encodePng();
        }
        finally {
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
        }
        finally {
            if (!pixmap.disposed)
                pixmap.dispose();
        }
    }
    const escapeXml = (s) => String(s)
        .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
        .replace(/"/g, '&quot;');
    function measureText(text, { fontSize = 12, family, ...options } = {}) {
        const fam = family ?? options.fontFamily ?? 'sans-serif';
        // `xml:space="preserve"`: the text as given, not with its whitespace
        // collapsed (`a  b` drew as `a b`).
        const probe = '<svg xmlns="http://www.w3.org/2000/svg">'
            + `<text id="m" xml:space="preserve" font-family="${escapeXml(fam)}" font-size="${escapeXml(fontSize)}">`
            + `${escapeXml(text)}</text></svg>`;
        const { tree } = parse(probe, { ...options, fontFamily: fam });
        try {
            const node = tree.nodeById('m');
            return (node && nodeExtent(node)?.width) ?? 0;
        }
        finally {
            tree.dispose();
        }
    }
    function fit(svg, render = {}) {
        if (typeof render === 'function')
            return fitTextWidths(svg, render);
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
