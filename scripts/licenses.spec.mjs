import test from 'node:test';
import assert from 'node:assert/strict';
import { copyrights, spdxIds, required, linkedIds, textFor, render } from './licenses.mjs';

test('copyrights', () => {
  assert.deepEqual(copyrights('Copyright (c) 2011 Google Inc. All rights reserved.\nblah'),
    ['Copyright (c) 2011 Google Inc. All rights reserved']);
  // The Apache template line names nobody and must not be reported as a holder.
  assert.deepEqual(copyrights('Copyright [yyyy] [name of copyright owner]'), []);
  // Nor the Apache prose lines that happen to wrap onto the word.
  assert.deepEqual(copyrights('copyright notice that is included in or attached to the work\ncopyright license to reproduce, prepare Derivative Works of,'), []);
  assert.deepEqual(copyrights('COPYRIGHT (C) 2020 A'), ['COPYRIGHT (C) 2020 A']);
  assert.equal(copyrights('Copyright (c) A\nCopyright (c) A\nCopyright (c) B').length, 2);
});

test('spdxIds', () => {
  assert.deepEqual(spdxIds('MIT OR Apache-2.0'), ['MIT', 'Apache-2.0']);
  assert.deepEqual(spdxIds('MIT/Apache-2.0'), ['MIT', 'Apache-2.0']);
  assert.ok(spdxIds('(MIT OR Apache-2.0) AND Unicode-3.0').includes('Unicode-3.0'));
});

test('required', () => {
  // A dual licence resolves to the text already shipped, not to a third file.
  assert.deepEqual(required('MIT OR Apache-2.0'), ['Apache-2.0']);
  assert.deepEqual(required('Zlib OR Apache-2.0 OR MIT'), ['Apache-2.0']);
  // A licence that stands alone is the one that must be quoted.
  assert.deepEqual(required('BSD-3-Clause'), ['BSD-3-Clause']);
  assert.deepEqual(required(undefined), ['UNKNOWN']);
  // AND is not a choice: the added terms come back with the chosen half...
  assert.deepEqual(required('(MIT OR Apache-2.0) AND Unicode-3.0'), ['Apache-2.0', 'Unicode-3.0']);
  // ...and no id ever carries a bracket.
  assert.ok(required('(MIT OR Apache-2.0) AND Unicode-3.0').every((id) => !/[()]/.test(id)));
});

test('linkedIds: build dependencies do not reach the binary', () => {
  const resolve = {
    nodes: [
      { id: 'root', deps: [
        { pkg: 'linked', dep_kinds: [{ kind: null }] },
        { pkg: 'builder', dep_kinds: [{ kind: 'build' }] },
        { pkg: 'tester', dep_kinds: [{ kind: 'dev' }] },
      ] },
      { id: 'linked', deps: [{ pkg: 'deep', dep_kinds: [{ kind: null }] }] },
      { id: 'builder', deps: [{ pkg: 'syn-ish', dep_kinds: [{ kind: null }] }] },
      { id: 'deep', deps: [] },
    ],
  };
  assert.deepEqual([...linkedIds(resolve, 'root')].sort(), ['deep', 'linked']);
  // A crate reached only through a build dependency stays out, however deep.
  assert.equal(linkedIds(resolve, 'root').has('syn-ish'), false);
  // A target-specific edge still ships: thirteen platforms come from one graph.
  assert.equal(linkedIds({ nodes: [{ id: 'r', deps: [
    { pkg: 'win-only', dep_kinds: [{ kind: null, target: 'cfg(windows)' }] }] }] }, 'r').has('win-only'), true);
});

test('textFor quotes the text named after the licence', () => {
  assert.equal(textFor([{ texts: [
    { file: 'LICENSE-APACHE', text: 'APACHE' }, { file: 'LICENSE-UNICODE', text: 'UNICODE' }] }],
    'Unicode-3.0'), 'UNICODE');
});

test('render', () => {
  const doc = render([
    { name: 'tiny-skia', version: '0.12.0', expr: 'BSD-3-Clause', ids: ['BSD-3-Clause'],
      holders: ['Copyright (c) 2011 Google Inc'], texts: [{ file: 'LICENSE', text: 'BSD TEXT HERE' }] },
    { name: 'serde', version: '1.0', expr: 'MIT OR Apache-2.0', ids: ['Apache-2.0'],
      holders: [], texts: [{ file: 'LICENSE-APACHE', text: 'APACHE TEXT' }] },
  ]);
  assert.match(doc, /## BSD-3-Clause/);
  assert.match(doc, /Copyright \(c\) 2011 Google Inc/);
  assert.match(doc, /BSD TEXT HERE/, 'a licence we do not otherwise ship is quoted in full');
  assert.doesNotMatch(doc, /APACHE TEXT/, 'Apache-2.0 is already in LICENSE-APACHE');
  assert.match(doc, /Offered as `MIT OR Apache-2\.0`/, 'a dual licence says which half was taken');
});
