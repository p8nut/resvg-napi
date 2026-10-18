// Points the generated WASI loaders at emnapi 2's own plugins.
//
// `napi build` writes four loaders that take emnapiAsyncWorkPlugin and
// emnapiTSFNPlugin from @napi-rs/wasm-runtime. Its 1.2.5 bundles copies of
// those plugins written before emnapi 2.0.0-beta.1 renamed the plugin context's
// `napiModule` to `addonModule`, so on a beta they throw while the module
// loads (`Cannot set properties of undefined (setting 'initWorker')`).
// @emnapi/core ships the same two plugins, current, as `@emnapi/core/plugins`:
// this rewrites the imports to take them from there.
//
// Run after every `napi build` (package.json's build scripts, CI's build step),
// so the committed loaders are the rewritten ones and the drift check compares
// like with like. Idempotent. Throws when a loader matches neither form, which
// is what a napi-rs release that changes them will do -- check then whether
// wasm-runtime reads `addonModule` itself, and delete this script if so.
//
//   node scripts/wasi-plugins.mjs              rewrite
//   node scripts/wasi-plugins.mjs --selftest   offline check of the rewrites

import { readFileSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));

/** [file, generated text, rewritten text]: plain substring swaps. */
const REWRITES = [
  [
    'resvg-napi.wasi.cjs',
    `const {
  emnapiAsyncWorkPlugin: __emnapiAsyncWorkPlugin,
  emnapiTSFNPlugin: __emnapiTSFNPlugin,
  createOnMessage: __wasmCreateOnMessageForFsProxy,`,
    `const { asyncWork: __emnapiAsyncWorkPlugin, tsfn: __emnapiTSFNPlugin } = require('@emnapi/core/plugins')
const {
  createOnMessage: __wasmCreateOnMessageForFsProxy,`,
  ],
  [
    'resvg-napi.wasi-browser.js',
    `import {
  emnapiAsyncWorkPlugin as __emnapiAsyncWorkPlugin,
  emnapiTSFNPlugin as __emnapiTSFNPlugin,
  createOnMessage as __wasmCreateOnMessageForFsProxy,`,
    `import { asyncWork as __emnapiAsyncWorkPlugin, tsfn as __emnapiTSFNPlugin } from '@emnapi/core/plugins'
import {
  createOnMessage as __wasmCreateOnMessageForFsProxy,`,
  ],
  [
    'wasi-worker-browser.mjs',
    `  WASI,
  emnapiAsyncWorkPlugin,
  emnapiTSFNPlugin,
} from '@napi-rs/wasm-runtime'`,
    `  WASI,
} from '@napi-rs/wasm-runtime'
import { asyncWork as emnapiAsyncWorkPlugin, tsfn as emnapiTSFNPlugin } from '@emnapi/core/plugins'`,
  ],
  [
    'wasi-worker.mjs',
    `    getDefaultContext,
    emnapiAsyncWorkPlugin,
    emnapiTSFNPlugin,
  } = require('@napi-rs/wasm-runtime')`,
    `    getDefaultContext,
  } = require('@napi-rs/wasm-runtime')
  const { asyncWork: emnapiAsyncWorkPlugin, tsfn: emnapiTSFNPlugin } = require('@emnapi/core/plugins')`,
  ],
];

/** The rewritten text, or a throw naming the file that matched neither form. */
export function rewrite(file, text, generated, rewritten) {
  if (text.includes(rewritten)) return text;
  if (!text.includes(generated)) {
    throw new Error(`${file}: neither the generated nor the rewritten plugin import -- see scripts/wasi-plugins.mjs`);
  }
  return text.replace(generated, rewritten);
}

if (process.argv.includes('--selftest')) {
  const assert = (await import('node:assert/strict')).default;
  for (const [file, generated, rewritten] of REWRITES) {
    const once = rewrite(file, `head\n${generated}\ntail`, generated, rewritten);
    assert.equal(once, `head\n${rewritten}\ntail`, `${file}: rewritten`);
    assert.equal(rewrite(file, once, generated, rewritten), once, `${file}: idempotent`);
    const fromRuntime = generated.split('\n').filter((l) => /Plugin/.test(l));
    assert.equal(fromRuntime.length, 2, `${file}: the generated form names both plugins`);
    for (const line of fromRuntime) assert.ok(!rewritten.split('\n').includes(line), `${file}: ${line.trim()} moved`);
    assert.match(rewritten, /@emnapi\/core\/plugins/, `${file}: taken from emnapi`);
    assert.throws(() => rewrite(file, 'something else', generated, rewritten), /neither/, `${file}: drift is loud`);
  }
  console.log(`ok — wasi-plugins: ${REWRITES.length * 7} checks passed`);
} else {
  for (const [file, generated, rewritten] of REWRITES) {
    const path = `${root}${file}`;
    const before = readFileSync(path, 'utf8');
    const after = rewrite(file, before, generated, rewritten);
    if (after !== before) writeFileSync(path, after);
  }
}
