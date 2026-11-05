// A font file's family name, read from its `name` table.
//
// The demo points the generic families (`sans-serif`...) at the first font it
// loads, so it needs that font's family as fontdb knows it. The namespaced
// `fontdb.FaceInfo` has no `families`: fontdb keeps them as
// `Vec<(String, Language)>`, a tuple the generator does not map. Until it
// does, this reads the same names fontdb reads, the same way
// (`fontdb::parse_names`): the typographic family (name ID 16), else the
// family (ID 1); Unicode and Windows records, English (US) first; a Mac Roman
// record only when no English (US) one exists.

const TYPOGRAPHIC_FAMILY = 16;
const FAMILY = 1;
const EN_US = 0x0409; // Windows language ID

/** UTF-16BE, as Unicode and Windows name records store it. */
function utf16be(bytes) {
  let s = '';
  for (let i = 0; i + 1 < bytes.length; i += 2) s += String.fromCharCode((bytes[i] << 8) | bytes[i + 1]);
  return s;
}

/** Mac Roman, close enough for a family name: its ASCII half is ASCII. */
const macRoman = (bytes) => String.fromCharCode(...bytes);

/** ttf-parser's `Name::is_unicode`. */
const isUnicode = (r) => r.platform === 0 || (r.platform === 3 && [0, 1, 10].includes(r.encoding));

/** English (US), as ttf-parser reads it from a Windows or a Mac record. */
const isEnUs = (r) => (r.platform === 3 && r.language === EN_US) || (r.platform === 1 && r.language === 0);

/** The name records of the first face, or `[]` when the file is not a font. */
function nameRecords(bytes) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  let face = 0;
  // A collection (`ttcf`): fontdb's first face is the first font.
  if (view.getUint32(0) === 0x74746366) face = view.getUint32(12);
  const tables = view.getUint16(face + 4);
  for (let i = 0; i < tables; i++) {
    const entry = face + 12 + i * 16;
    if (view.getUint32(entry) !== 0x6e616d65) continue; // 'name'
    const table = view.getUint32(entry + 8);
    const count = view.getUint16(table + 2);
    const strings = table + view.getUint16(table + 4);
    const records = [];
    for (let j = 0; j < count; j++) {
      const at = table + 6 + j * 12;
      const length = view.getUint16(at + 8);
      const offset = strings + view.getUint16(at + 10);
      records.push({
        platform: view.getUint16(at),
        encoding: view.getUint16(at + 2),
        language: view.getUint16(at + 4),
        id: view.getUint16(at + 6),
        bytes: bytes.subarray(offset, offset + length),
      });
    }
    return records;
  }
  return [];
}

function families(records, id) {
  const found = records.filter((r) => r.id === id && isUnicode(r))
    .map((r) => ({ name: utf16be(r.bytes), enUs: isEnUs(r) }));
  if (!found.some((f) => f.enUs)) {
    const mac = records.find((r) => r.id === id && r.platform === 1 && r.encoding === 0);
    if (mac) found.push({ name: macRoman(mac.bytes), enUs: isEnUs(mac) });
  }
  return found;
}

/**
 * The family fontdb gives the first face of a font file (`families[0]`), or
 * `undefined` when the bytes are not a font it would read.
 *
 * @param {Uint8Array} bytes  a TTF, OTF or TTC file
 */
export function familyName(bytes) {
  let records;
  try {
    records = nameRecords(bytes);
  } catch {
    return undefined; // too short, or not a font
  }
  let found = families(records, TYPOGRAPHIC_FAMILY);
  if (!found.length) found = families(records, FAMILY);
  return (found.find((f) => f.enUs) ?? found[0])?.name;
}
