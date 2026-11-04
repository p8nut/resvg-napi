// What `console.log` shows of a class instance: its values live in prototype
// getters and methods, so without `util.inspect.custom` every one printed `{}`.
import assert from 'node:assert/strict';
import { Resvg, FontDatabase } from '../index.js';
import { inspect } from 'node:util';

const doc = new Resvg(`<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20">
  <g id="a" opacity=".5"><rect id="r" width="5" height="5"/></g>
</svg>`);
const group = doc.children()[0];
const rect = group.children()[0];

// 1. getters and cheap methods, under the class name
assert.match(inspect(doc), /^Resvg \{\n  width: 10,\n  height: 20,/);
assert.match(inspect(group), /^SvgNode \{\n  kind: 'group',\n  id: 'a',\n  opacity: 0\.5,/);
assert.match(inspect(new FontDatabase()), /^FontDatabase \{ isEmpty: true, len: 0 \}$/);

// 2. a member that does not apply is left out rather than printed as null
assert.equal(inspect(rect), "SvgNode { kind: 'path', id: 'r' }");

// 3. depth behaves as for any object: an instance past the limit is its name
assert.equal(inspect([{ x: 1 }], { depth: 0 }), '[ [Object] ]');
assert.equal(inspect([rect], { depth: 0 }), '[ [SvgNode] ]');
assert.equal(inspect([rect], { depth: 1 }), "[ SvgNode { kind: 'path', id: 'r' } ]");
assert.equal(inspect({ a: { b: rect } }, { depth: 1 }), "{ a: { b: [SvgNode] } }");
assert.match(inspect(doc, { depth: null }), /^Resvg \{/);

// 4. Object.prototype.toString names the class
assert.equal(Object.prototype.toString.call(rect), '[object SvgNode]');
assert.equal(String(rect), '[object SvgNode]');
