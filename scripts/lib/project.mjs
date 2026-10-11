// The names the scripts need, read from the manifests instead of repeated in
// each of them. A script that names the package itself breaks the day it is
// copied into another napi-rs project; one that imports this does not.
import { readFileSync } from 'node:fs';

const read = (f) => readFileSync(new URL(`../../${f}`, import.meta.url), 'utf8');
const pkg = JSON.parse(read('package.json'));

/** The npm name: also the prefix of every platform package. */
export const npmName = pkg.name;

/** The file stem napi gives the binaries and the WASI shims. */
export const binaryName = pkg.napi?.binaryName ?? pkg.name;

/** The Cargo package name, as cargo prints it in `warning: <name>@<version>:`. */
export const crateName = read('Cargo.toml').match(/^\[package\][^[]*?^name\s*=\s*"([^"]+)"/ms)[1];
