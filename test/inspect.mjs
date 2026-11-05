// What `console.log` shows of a class instance: its values live in prototype
// getters and methods, so without `util.inspect.custom` every one printed `{}`.
import assert from 'node:assert/strict';
import { fontdb, tiny_skia, usvg } from '../index.js';
import { inspect } from 'node:util';

const svg = `<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20">
  <g id="a" opacity=".5"><rect id="r" width="5" height="5"/></g>
</svg>`;
const tree = usvg.Tree.fromStr(svg);
const group = tree.root().children()[0].value;
const rect = group.children()[0].value;

// 1. getters and cheap methods, under the class name
assert.match(inspect(tree), /^Tree \{/);
assert.match(inspect(group), /^Group \{\n  id: 'a',\n  opacity: 0\.5,/);
assert.match(inspect(new fontdb.Database()), /^Database \{ isEmpty: true, len: 0 \}$/);
assert.match(inspect(tiny_skia.NonZeroRect.fromXywh(1, 2, 3, 4)), /^NonZeroRect \{.*x: 1,.*width: 3,/s);

// 2. a member that does not apply is left out rather than printed as null:
//    a path has no opacity of its own
assert.doesNotMatch(inspect(rect), /opacity/);
assert.match(inspect(rect), /^Path \{\n  id: 'r',/);

// 3. depth behaves as for any object: an instance past the limit is its name
assert.equal(inspect([{ x: 1 }], { depth: 0 }), '[ [Object] ]');
assert.equal(inspect([rect], { depth: 0 }), '[ [Path] ]');
assert.match(inspect([rect], { depth: 1 }), /^\[\n {2}Path \{\n {4}id: 'r',/);
assert.equal(inspect({ a: { b: rect } }, { depth: 1 }), '{ a: { b: [Path] } }');
assert.match(inspect(tree, { depth: null }), /^Tree \{/);

// 4. Object.prototype.toString names the class
assert.equal(Object.prototype.toString.call(rect), '[object Path]');
assert.equal(String(rect), '[object Path]');
