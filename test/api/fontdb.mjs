// fontdb's members that take or hand out a face ID, a query or a callback
// (#181): `fontdb.ID` is an opaque class held by value, `fontdb.Query` an
// option object borrowing a list of `fontdb.Family` (`{ type, value }`),
// `withFaceData` returns what its closure was handed, copied, and a face's
// `fontdb.Source` is a union out and an input enum in.
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { inspect } from 'node:util';
import binding from '../../index.js';

const { fontdb, usvg } = binding;
const FONT_URL = new URL('../../demo/fonts/IBMPlexSans-Latin.ttf', import.meta.url);
const FONT = readFileSync(FONT_URL);
const FAMILY = 'IBM Plex Sans';

function database() {
  const db = new fontdb.Database();
  db.loadFontData(FONT);
  return db;
}

test('fontdb.ID: handed out by FaceInfo.id, passed back to face(id)', () => {
  const db = database();
  const [info] = db.faces();
  const id = info.id;
  assert.ok(id instanceof fontdb.ID);
  assert.match(String(id), /^\d+$/);
  assert.equal(db.face(id).postScriptName, info.postScriptName);
  assert.equal(String(db.face(id).id), String(id), 'the same ID, printed the same');
  assert.equal(db.face(fontdb.ID.dummy()), null, 'no face for a dummy ID');
});

test('query: a list of families, weight, stretch and style over Default', () => {
  const db = database();
  const id = String(db.faces()[0].id);
  assert.equal(String(db.query({ families: [{ type: 'name', value: FAMILY }] })), id);
  assert.equal(String(db.query({ families: [{ type: 'name', value: 'Nope' }, { type: 'name', value: FAMILY }] })), id);
  assert.equal(db.query({ families: [{ type: 'name', value: 'Nope' }] }), null);
  assert.equal(db.query(), null, 'no families: nothing matches');
  // A generic family is the database's own setting.
  assert.equal(db.query({ families: [{ type: 'serif' }] }), null);
  db.setSerifFamily(FAMILY);
  assert.equal(String(db.query({ families: [{ type: 'serif' }], weight: 700, stretch: 'normal', style: 'italic' })), id);
  assert.throws(() => db.query({ weight: 70000 }), { code: 'InvalidArg', message: 'invalid weight: 70000' });
  assert.throws(() => db.query({ families: [{ type: 'name' }] }), { code: 'InvalidArg', message: "Family 'name' needs a value" });
});

test('familyName: a named family is itself, a generic one the database\'s', () => {
  const db = new fontdb.Database();
  assert.equal(db.familyName({ type: 'name', value: 'Arial' }), 'Arial');
  assert.equal(db.familyName({ type: 'serif' }), 'Times New Roman');
  db.setMonospaceFamily('Mono X');
  assert.equal(db.familyName({ type: 'monospace' }), 'Mono X');
  assert.throws(() => db.familyName({ type: 'nope' }), { code: 'InvalidArg', message: 'invalid Family type: "nope"' });
});

test('withFaceData: the font file and its face index, copied out', () => {
  const db = database();
  const id = db.faces()[0].id;
  const [data, index] = db.withFaceData(id);
  assert.ok(Buffer.isBuffer(data));
  assert.ok(data.equals(FONT));
  assert.equal(index, 0);
  assert.equal(db.withFaceData(fontdb.ID.dummy()), null);
});

test('removeFace and makeFaceDataUnshared take an ID', () => {
  const db = database();
  const id = db.faces()[0].id;
  // fontdb's `memmap` is off on WASI: the member throws there, by design.
  if (binding.__napiBindingTarget?.startsWith('wasm32')) {
    assert.throws(() => db.makeFaceDataUnshared(id), /upstream feature is off/);
  } else {
    db.makeFaceDataUnshared(id);
  }
  assert.equal(db.len(), 1);
  db.removeFace(id);
  assert.equal(db.len(), 0);
  assert.equal(db.face(id), null);
});

test('PositionedGlyph.font is the ID of the face that drew it', () => {
  const db = database();
  const tree = usvg.Tree.fromStr('<svg xmlns="http://www.w3.org/2000/svg" width="80" height="30"><text id="t" x="2" y="20">Hi</text></svg>',
    { fontdb: db, fontFamily: FAMILY });
  const [glyph] = tree.nodeById('t').value.layouted()[0].positionedGlyphs;
  assert.ok(glyph.font instanceof fontdb.ID);
  assert.equal(db.face(glyph.font).postScriptName, 'IBMPlexSans-Regular');
});

test('Source out: a face\'s bytes, or the file it is read from', () => {
  const db = database();
  const [info] = db.faces();
  assert.equal(info.source.type, 'binary');
  assert.ok(info.source.bytes.equals(FONT));
  const [source, index] = db.faceSource(info.id);
  assert.equal(source.type, 'binary');
  assert.ok(source.bytes.equals(FONT));
  assert.equal(index, 0);
  assert.equal(db.faceSource(fontdb.ID.dummy()), null);
  // Printing reads no bytes getter: each read copies the whole font.
  assert.equal(inspect(source), "SourceBinary { type: 'binary' }");
  assert.doesNotMatch(inspect(info, { depth: 3 }), /Buffer/);

  const path = fileURLToPath(FONT_URL);
  const files = new fontdb.Database();
  files.loadFontFile(path);
  assert.deepEqual(files.faces()[0].source, { type: 'file', value: path });
});

test('Source in: loadFontSource takes { type, bytes } or { type, value }', () => {
  const db = new fontdb.Database();
  const [id] = db.loadFontSource({ type: 'binary', bytes: FONT });
  assert.ok(id instanceof fontdb.ID);
  assert.ok(db.face(id).source.bytes.equals(FONT));
  // What faceSource hands out goes back in.
  const ids = db.loadFontSource(db.faceSource(id)[0]);
  assert.equal(ids.length, 1);
  assert.equal(db.len(), 2);

  const path = fileURLToPath(FONT_URL);
  const [fid] = db.loadFontSource({ type: 'file', value: path });
  assert.deepEqual(db.face(fid).source, { type: 'file', value: path });
  assert.ok(db.withFaceData(fid)[0].equals(FONT), 'read from the file');
  assert.deepEqual(db.loadFontSource({ type: 'binary', bytes: new Uint8Array([1, 2, 3]) }), [], 'not a font: no face');

  assert.throws(() => db.loadFontSource({ type: 'binary' }), { code: 'InvalidArg', message: "Source 'binary' needs bytes" });
  assert.throws(() => db.loadFontSource({ type: 'file' }), { code: 'InvalidArg', message: "Source 'file' needs a value" });
  assert.throws(() => db.loadFontSource({ type: 'sharedFile', value: path, bytes: FONT }),
    { code: 'InvalidArg', message: "Source 'sharedFile' cannot be passed in: it carries several values" });
  assert.throws(() => db.loadFontSource({ type: 'nope' }), { code: 'InvalidArg' });
});
