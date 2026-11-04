// Injected into the resvg.js bundle by build.mjs, so it runs before the loader.
//
// napi's `Buffer` return type is Node's Buffer, and emnapi looks it up on
// globalThis -- browsers have no such thing. Node's Buffer is a Uint8Array
// subclass, and emnapi only touches `from`, `alloc` and `isBuffer`, so this
// is enough.
class BufferPolyfill extends Uint8Array {
  static from(src, offset, length) {
    // With wasm threads the module's memory is a SharedArrayBuffer, not an
    // ArrayBuffer -- missing that case yields empty views.
    const isBuf = src instanceof ArrayBuffer
      || (typeof SharedArrayBuffer !== 'undefined' && src instanceof SharedArrayBuffer);
    return isBuf
      ? new BufferPolyfill(src, offset, length)
      : new BufferPolyfill(Uint8Array.from(src));
  }
  static alloc(size) { return new BufferPolyfill(size); }
  static isBuffer(value) { return value instanceof BufferPolyfill; }
}
globalThis.Buffer ??= BufferPolyfill;
