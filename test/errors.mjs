// A parse failure carries usvg's own variant name as `error.code`, on every
// path that parses: the constructor, parseAsync and the one-shot renderAsync.
import assert from 'node:assert/strict';
import { createRequire } from 'node:module';
const { Resvg, renderAsync, FontDatabase } = createRequire(import.meta.url)('../index.js');

const cases = {
  NotAnUtf8Str: Buffer.from([0xff, 0xfe, 0x3c]),
  MalformedGZip: Buffer.from([0x1f, 0x8b, 1, 2]),
  InvalidSize: '<svg xmlns="http://www.w3.org/2000/svg" width="0" height="0"/>',
  ParsingFailed: '<svg',
};

for (const [code, svg] of Object.entries(cases)) {
  const is = (e) => e instanceof Error && e.code === code && /^invalid SVG: /.test(e.message);
  assert.throws(() => new Resvg(svg), is, `new Resvg: ${code}`);
  await assert.rejects(Resvg.parseAsync(svg), is, `parseAsync: ${code}`);
  await assert.rejects(renderAsync(svg), is, `renderAsync: ${code}`);
}


// An instance of another class where a class is expected is refused, not
// reinterpreted: unchecked, a SvgNode read as a FontFace is memory corruption.
// Native and WASI alike (npm run test:wasi).
{
  const node = new Resvg('<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"><rect id="r" width="1" height="1"/></svg>').node('r');
  assert.ok(node, 'the rect survives parsing');
  const db = new FontDatabase();
  const is = (e) => e instanceof Error && e.code === 'InvalidArg' && /not an instance of class `FontFace`/.test(e.message);
  assert.throws(() => db.face(node), is, 'face');
  assert.throws(() => db.faceSource(node), is, 'faceSource');
  assert.throws(() => db.removeFace(node), is, 'removeFace');
}
