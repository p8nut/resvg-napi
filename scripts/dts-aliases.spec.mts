import test from 'node:test';
import assert from 'node:assert/strict';
import { dropAliases } from './dts-aliases.mts';

test('drops the alias napi prints after a class, and nothing else', () => {
  const text = [
    'export declare namespace usvg {',
    '  export class Tree {',
    '    size(): number',
    '  }',
    '  export type UsvgTree = Tree',
    '  export interface Size {',
    '    width: number',
    '  }',
    '  export type Other = Size',
    '  export type Node = usvg.NodeGroup | usvg.NodePath',
    '}',
    'export declare class Counter {',
    '}',
    'export type ToyCounter = Counter',
    '',
  ].join('\n');
  assert.equal(
    dropAliases(text),
    [
      'export declare namespace usvg {',
      '  export class Tree {',
      '    size(): number',
      '  }',
      '  export interface Size {',
      '    width: number',
      '  }',
      '  export type Other = Size',
      '  export type Node = usvg.NodeGroup | usvg.NodePath',
      '}',
      'export declare class Counter {',
      '}',
      '',
    ].join('\n'),
  );
});
