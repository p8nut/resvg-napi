import test from 'node:test';
import assert from 'node:assert/strict';
import { normalise } from './codegen-report.mjs';

test('ours only, sorted, and the registry path reduced to the crate dir', () => {
  const raw = [
    'warning: resvg-napi@0.1.0: usvg sources: /home/x/.cargo/registry/src/idx-abc/usvg-0.48.1/src',
    'warning: resvg-napi@0.1.0: b line',
    'warning: some other crate: not ours',
    'plain cargo noise',
  ].join('\n');
  assert.equal(normalise(raw), 'b line\nusvg sources: usvg-0.48.1\n');
  // The version survives, so a bump is a diff rather than a silence.
  assert.notEqual(normalise(raw.replace('0.48.1', '0.49.0')), normalise(raw));
});

test('a colour-decorated line still counts', () => {
  assert.equal(
    normalise('\u001b[1m\u001b[33mwarning\u001b[0m\u001b[1m: resvg-napi@0.1.0: coloured\u001b[0m'),
    'coloured\n',
  );
});

test('a Windows path reduces the same way', () => {
  assert.match(
    normalise('warning: resvg-napi@0.1.0: usvg sources: C:\\c\\registry\\src\\i\\usvg-1.0.0\\src'),
    /usvg sources: usvg-1\.0\.0/,
  );
});
