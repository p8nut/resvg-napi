// Horizontal fitting for text that must not exceed a width, the way an
// Illustrator template expresses it: `data-maxwidth` on the element itself.
//
// resvg drops attributes it does not know, so the constraint can only be read
// from the source. The measurement, on the other hand, comes from the render:
// `node(id).extent()` is the width the text actually occupies with the fonts in
// hand -- which is the only number that matters.
//
// The correction is a horizontal scale appended to the element's own transform,
// matching what these templates already do (`scale(1.11 1)`). A pure geometric
// scale is exactly linear in width, so one pass lands on the target; the second
// measurement is a check, not an iteration.

const ATTR = 'data-maxwidth';

/**
 * Ratio between the tree's canvas units and the document's own coordinates.
 *
 * `width="85.6mm" viewBox="0 0 240.94 …"` makes usvg normalise the tree to the
 * physical size (323.53 units at 96 dpi), so `extent()` comes back 1.343× larger
 * than the numbers written in the file. A limit expressed in the document's own
 * units has to be compared in those units, or everything wider than
 * `limit / ratio` gets compressed while it actually fits.
 */
function unitScale(svg, canvasWidth) {
  const viewBox = svg.match(/\bviewBox\s*=\s*"\s*[-\d.]+[\s,]+[-\d.]+[\s,]+([\d.]+)/);
  const width = viewBox ? Number(viewBox[1]) : NaN;
  if (!Number.isFinite(width) || width <= 0) return 1;
  return canvasWidth / width;
}

/** Elements carrying a width constraint, with the tag text needed to rewrite it. */
export function findWidthConstraints(svg) {
  const found = [];
  for (const match of svg.matchAll(/<([a-zA-Z][\w:.-]*)\b([^>]*?)(\/?)>/g)) {
    const [tag, name, attrs] = match;
    const max = attrs.match(new RegExp(`${ATTR}\\s*=\\s*"([^"]*)"`))?.[1];
    if (max === undefined) continue;
    const width = Number(max);
    const id = attrs.match(/\bid\s*=\s*"([^"]*)"/)?.[1];
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
  const anchor = Number.parseFloat(attrs.match(/\bx\s*=\s*"\s*([-\d.]+)/)?.[1] ?? '0') || 0;
  const scale = anchor
    ? `translate(${round(anchor)} 0) scale(${round(factor)} 1) translate(${round(-anchor)} 0)`
    : `scale(${round(factor)} 1)`;
  const existing = attrs.match(/\btransform\s*=\s*"([^"]*)"/);
  if (existing) {
    // Composed on the right: the element keeps its position, the glyphs
    // compress toward the anchor the transform established.
    return tag.replace(existing[0], `transform="${existing[1].trim()} ${scale}"`);
  }
  return tag.replace(/(\/?)>$/, ` transform="${scale}"$1>`);
}

const round = (n) => Math.round(n * 10000) / 10000;

/**
 * Measuring goes through `node(id)`, but a template has no reason to carry ids
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
 * Fits every `data-maxwidth` element by compressing it horizontally.
 *
 * @param svg     source, after any templating
 * @param render  (svg) => Resvg instance; supply fonts and options here
 * @returns { svg, adjustments, problems }
 */
export function fitTextWidths(svg, render) {
  const problems = [];
  if (!findWidthConstraints(svg).length) return { svg, adjustments: [], problems };

  // Every constrained element carries an id from here on, and the offsets are
  // read from this exact string.
  svg = ensureIds(svg);
  const constraints = findWidthConstraints(svg);

  let doc;
  try {
    doc = render(svg);
  } catch (e) {
    return { svg, adjustments: [], problems: [`could not measure: ${e.message}`] };
  }

  // Every width below is in the document's own units, like the attribute.
  const scale = unitScale(svg, doc.width);
  let out = svg;
  const adjustments = [];
  const splices = [];
  for (const c of constraints) {
    if (!Number.isFinite(c.max) || c.max <= 0) {
      problems.push(`#${c.id}: ${ATTR}="${c.max}" is not a usable width`);
      continue;
    }
    const extent = doc.node(c.id)?.extent();
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
    const afterScale = unitScale(out, after.width);
    for (const a of adjustments) {
      const got = after.node(a.id)?.extent()?.width;
      a.measured = got === undefined ? NaN : round(got / afterScale);
    }
  } catch (e) {
    problems.push(`could not verify: ${e.message}`);
  }
  return { svg: out, adjustments, problems };
}
