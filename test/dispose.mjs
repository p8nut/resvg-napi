// `dispose()` frees a document now, and `using` calls it through
// `[Symbol.dispose]`. Every method after it throws rather than answering
// from the empty tree left in its place.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
import { inspect } from 'node:util';
const { Resvg } = createRequire(import.meta.url)('../index.js');

const svg = '<svg xmlns="http://www.w3.org/2000/svg" width="10" height="20"><rect id="r" width="5" height="5"/></svg>';

// 1. the alias, set at load where the symbol exists (Node 20.4+ defines it;
//    the `using` syntax that calls it is Node 24+, so not used here)
assert.equal(Resvg.prototype[Symbol.dispose], Symbol.dispose && Resvg.prototype.dispose);

// 2. what `using` calls at the end of its block
const doc = new Resvg(svg);
assert.equal(doc.disposed, false);
(Symbol.dispose ? doc[Symbol.dispose] : doc.dispose).call(doc);
assert.equal(doc.disposed, true);

// 3. getters, methods and async twins throw, synchronously
for (const call of [() => doc.width, () => doc.renderPng(), () => doc.node('r'), () => doc.renderPngAsync()]) {
  assert.throws(call, { message: 'this Resvg was disposed' });
}

// 4. console.log still works, and a second dispose is harmless
assert.equal(inspect(doc), 'Resvg { disposed: true }');
doc.dispose();

// 5. a node taken before keeps its own reference to the tree
const live = new Resvg(svg);
const node = live.node('r');
live.dispose();
assert.equal(node.kind, 'path');
