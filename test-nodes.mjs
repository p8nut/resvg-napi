// SvgNode: read-only handles on inner elements (Arc<Tree> + index path).
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const { Resvg } = createRequire(import.meta.url)('./index.js');

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200">
  <filter id="f"><feGaussianBlur stdDeviation="5"/></filter>
  <g id="wrap" filter="url(#f)">
    <rect id="r" x="50" y="50" width="40" height="40" fill="red"/>
  </g>
  <g id="outer"><g id="mid"><rect id="deep" x="5" y="5" width="10" height="10" fill="blue"/></g></g>
</svg>`;
const r = new Resvg(svg);
const box = (b) => b && [b.x, b.y, b.width, b.height];

// 1. walking from the root
const top = r.children();
assert.deepEqual(top.map((n) => [n.kind, n.id()]), [['group', 'wrap'], ['group', 'outer']]);
const [wrap] = top;
const [rect] = wrap.children();
assert.equal(rect.kind, 'path');
assert.deepEqual(rect.children(), [], 'a path has no children');

// 2. index paths resolve at depth
const deep = r.children()[1].children()[0].children()[0];
assert.deepEqual([deep.kind, deep.id()], ['path', 'deep']);
assert.deepEqual(box(deep.absBoundingBox()), [5, 5, 10, 10]);

// 3. lookup by id, from anywhere in the tree
assert.deepEqual(box(r.node('deep').absBoundingBox()), [5, 5, 10, 10]);
assert.equal(r.node('nope'), null);

// 4. the limitation the wrapper exists to fix: inner-group filter extent
assert.equal(r.filtersBoundingBox(), null, 'the root group carries no filter');
assert.deepEqual(box(wrap.filtersBbox()), [46, 46, 48, 48], 'blur spill, seen from the group');
assert.equal(rect.filtersBbox(), null);

// 5. extent (rendered) vs geometry
assert.deepEqual(box(rect.absBoundingBox()), [50, 50, 40, 40]);
assert.deepEqual(box(wrap.extent()), [46, 46, 48, 48], 'filter widens the group extent');

// 6. per-node render agrees with the id-based entry point, byte for byte
assert.deepEqual([...r.node('r').renderPng()], [...r.renderNodePng('r')]);
assert.ok(rect.renderPng({ scale: 3 }).length > rect.renderPng().length);

// 7. clip path / mask of an element, matched by id against the defs tables
const clipped = new Resvg(`<svg xmlns="http://www.w3.org/2000/svg" width="60" height="40">
  <defs><clipPath id="cp"><rect width="30" height="40"/></clipPath>
        <mask id="mk"><rect width="40" height="40" fill="white"/></mask></defs>
  <g id="a" clip-path="url(#cp)"><rect width="60" height="40" fill="red"/></g>
  <g id="b" mask="url(#mk)"><rect width="60" height="40" fill="blue"/></g>
  <rect id="plain" width="10" height="10"/>
</svg>`);
assert.equal(clipped.node('a').clipPath().id(), 'cp');
assert.equal(clipped.node('a').mask(), null);
assert.equal(clipped.node('b').mask().id(), 'mk');
assert.equal(clipped.node('b').mask().kind(), 'luminance');
assert.equal(clipped.node('plain').clipPath(), null, 'a path is not a clipped group');

// 8. text nodes are reachable too
const t = new Resvg('<svg xmlns="http://www.w3.org/2000/svg" width="60" height="30"><text id="t" x="2" y="20" font-size="12">hi</text></svg>',
  { fontFamily: 'DejaVu Sans' });
assert.equal(t.node('t').kind, 'text');
assert.ok(t.node('t').extent().width > 0);

console.log('ok — node wrappers: all checks passed');
