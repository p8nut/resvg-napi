// What 0.10 left out of the old `SvgNode` surface (#191), back through generic
// rules: a payload variant carrying a class hands out a copy of it (the tree an
// `<image>` embeds, what an `feImage` draws), `usvg.Font.families` is read
// from the enum usvg re-exports from svgtypes, and a glyph's `font` is the
// `fontdb.ID` of the face that drew it.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import binding from '../../index.js';

const { fontdb, usvg, resvg, tiny_skia } = binding;
const FONT = readFileSync(new URL('../../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url));

/** The first node of `kind` under `group`, depth first. */
function find(group, kind) {
  for (const n of group.children()) {
    if (n.kind === kind) return n.value;
    if (n.kind === 'group') {
      const found = find(n.value, kind);
      if (found) return found;
    }
  }
  return undefined;
}

/** Every group of the tree, the root first. */
function groups(tree) {
  const out = [];
  const walk = (g) => {
    out.push(g);
    for (const n of g.children()) if (n.kind === 'group') walk(n.value);
  };
  walk(tree.root());
  return out;
}

test('an <image> of an SVG: kind().value is the tree it embeds', () => {
  const inner = '<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4"><rect id="r" width="4" height="4" fill="lime"/></svg>';
  const href = `data:image/svg+xml;base64,${Buffer.from(inner).toString('base64')}`;
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8">
    <image href="${href}" width="8" height="8"/></svg>`);
  const kind = find(tree.root(), 'image').kind();
  assert.equal(kind.type, 'svg');
  assert.ok(kind.value instanceof usvg.Tree);
  assert.equal(kind.value.size().width(), 4);
  assert.equal(kind.value.nodeById('r').kind, 'path');
  // A tree like any other: it renders, and outlives the document it came from.
  tree.dispose();
  const pixmap = new tiny_skia.Pixmap(4, 4);
  resvg.render(kind.value, tiny_skia.Transform.identity(), pixmap);
  assert.deepEqual([...pixmap.data().subarray(0, 4)], [0, 255, 0, 255]);
});

test('an feImage: kind.value.root is the group it draws', () => {
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="10" height="10">
    <filter id="f"><feImage xlink:href="#src"/></filter>
    <rect id="src" width="5" height="5" fill="blue"/>
    <rect width="10" height="10" filter="url(#f)"/></svg>`);
  const [primitive] = groups(tree).flatMap((g) => g.filters())[0].primitives();
  const { kind } = primitive;
  assert.equal(kind.type, 'image');
  assert.ok(kind.value instanceof usvg.FilterImage);
  const root = kind.value.root;
  assert.ok(root instanceof usvg.Group);
  assert.deepEqual(root.children().map((n) => n.kind), ['path']);
  // A copy of what it draws: its rect renders on its own.
  const pixmap = new tiny_skia.Pixmap(10, 10);
  resvg.renderNode(root.children()[0], tiny_skia.Transform.identity(), pixmap);
  assert.deepEqual([...pixmap.data().subarray(0, 4)], [0, 0, 255, 255]);
});

test('usvg.Font.families: the font-family list, generic and named', () => {
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">
    <text font-family="'IBM Plex Sans', Nope, serif, monospace">hi</text></svg>`, { fontdb: db });
  const [chunk] = find(tree.root(), 'text').chunks();
  assert.deepEqual(chunk.spans[0].font.families, [
    { type: 'named', value: 'IBM Plex Sans' },
    { type: 'named', value: 'Nope' },
    { type: 'serif' },
    { type: 'monospace' },
  ]);
});

test('PositionedGlyph.font: the face that drew the glyph, for db.face(id)', () => {
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  const tree = usvg.Tree.fromStr(`<svg xmlns="http://www.w3.org/2000/svg" width="40" height="20">
    <text x="2" y="15" font-family="Nope, IBM Plex Sans">hi</text></svg>`, { fontdb: db });
  const [span] = find(tree.root(), 'text').layouted();
  const glyph = span.positionedGlyphs[0];
  assert.ok(glyph.font instanceof fontdb.ID);
  const face = db.face(glyph.font);
  assert.equal(String(face.id), String(db.faces()[0].id));
  assert.deepEqual(face.families.map(([name]) => name), ['IBM Plex Sans']);
});
