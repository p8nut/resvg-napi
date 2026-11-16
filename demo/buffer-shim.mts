// Injected into the resvg.js bundle by build.mts, so it runs before the loader.
//
// napi's `Buffer` return type is Node's Buffer, and emnapi looks it up on
// globalThis -- browsers have no such thing. Node's Buffer is a Uint8Array
// subclass, and emnapi only touches `from`, `alloc` and `isBuffer`, so this
// is enough.

/** `v instanceof SharedArrayBuffer`, where the runtime has one. */
const isShared = (v: unknown): v is SharedArrayBuffer =>
  typeof SharedArrayBuffer !== 'undefined' && v instanceof SharedArrayBuffer;

// @ts-expect-error -- `from(buffer, offset, length)` is Node's Buffer.from, which emnapi calls; Uint8Array.from's (source, mapfn, thisArg) is not what this class offers.
class BufferPolyfill extends Uint8Array {
  static from(src: ArrayBuffer | SharedArrayBuffer | ArrayLike<number>, offset?: number, length?: number): BufferPolyfill {
    // With wasm threads the module's memory is a SharedArrayBuffer, not an
    // ArrayBuffer -- missing that case yields empty views.
    const isBuf = src instanceof ArrayBuffer || isShared(src);
    return isBuf
      ? new BufferPolyfill(src as ArrayBuffer, offset, length)
      : new BufferPolyfill(Uint8Array.from(src));
  }
  static alloc(size: number): BufferPolyfill { return new BufferPolyfill(size); }
  static isBuffer(value: unknown): value is BufferPolyfill { return value instanceof BufferPolyfill; }
}
// @ts-expect-error -- only what emnapi calls, not all of Node's BufferConstructor.
globalThis.Buffer ??= BufferPolyfill;
