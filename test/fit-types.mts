// Compile-time check that fit.d.mts describes fit.mjs. It is hand-written, so
// unlike index.d.ts it can drift -- and a declaration that lies about a shape is
// worse than none, because it type-checks the wrong code.
import { findWidthConstraints, fitTextWidths } from '../fit.mjs';
import type { WidthConstraint, Adjustment, FitResult } from '../fit.d.mts';

const found: WidthConstraint[] = findWidthConstraints('<svg/>');
for (const c of found) {
  const _tag: string = c.tag;
  const _name: string = c.name;
  const _attrs: string = c.attrs;
  const _max: number = c.max;
  const _index: number = c.index;
  const _id: string | null = c.id;
  console.log(_tag, _name, _attrs, _max, _index, _id);
}

const out: FitResult = fitTextWidths('<svg/>', () => ({ node: () => null }));
const _svg: string = out.svg;
const _problems: string[] = out.problems;
for (const a of out.adjustments as Adjustment[]) {
  console.log(a.id, a.from, a.to, a.factor, a.measured);
}

// @ts-expect-error the render callback must return something with `node`
fitTextWidths('<svg/>', () => 42);
