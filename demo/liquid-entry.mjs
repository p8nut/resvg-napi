// Bundle entry for the demo: LiquidJS plus the `qr` filter's implementation.
import qrcode from 'qrcode-generator';

export { Liquid } from 'liquidjs';
export { fitTextWidths } from '../fit.mjs';

/**
 * QR code as an SVG fragment, sized to whatever viewport it is dropped into.
 *
 * Returns a nested `<svg>` with a viewBox and `width/height="100%"`, so
 * `<svg id="qr" x=".." y=".." width="39.68" height="39.68">{{ upn | qr }}</svg>`
 * scales it without the template having to know the module count.
 *
 * @param text  value to encode
 * @param dark  colour of the modules (default black)
 * @param light colour behind them; `#00000000` for transparent (default none)
 * @param level error correction: L, M, Q or H (default M)
 */
export function qrSvg(text, dark = '#000000', light = 'none', level = 'M') {
  const value = String(text ?? '');
  if (!value) return '';

  // typeNumber 0 = smallest version that fits the data
  const qr = qrcode(0, level);
  qr.addData(value);
  qr.make();

  const n = qr.getModuleCount();
  // One path for every dark module: a single `d` beats thousands of <rect>s,
  // and crispEdges keeps the modules square at any scale.
  let d = '';
  for (let row = 0; row < n; row++) {
    for (let col = 0; col < n; col++) {
      if (qr.isDark(row, col)) d += `M${col} ${row}h1v1h-1z`;
    }
  }

  const background = light && light !== 'none'
    ? `<rect width="${n}" height="${n}" fill="${light}"/>`
    : '';
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 ${n} ${n}"`
    + ` width="100%" height="100%" shape-rendering="crispEdges">`
    + `${background}<path d="${d}" fill="${dark}"/></svg>`;
}
