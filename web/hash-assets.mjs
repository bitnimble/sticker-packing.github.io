// Content-hash the generated engine files for the Pages deploy so every build produces unique URLs
// -- GitHub Pages' CDN caches by filename, so fixed names (app.js / worker.js / *.wasm) can serve a
// stale engine after a deploy. index.html keeps its stable name; everything it (transitively) loads
// is hashed and its references rewritten. Local dev is untouched: it serves the plain-named files.
//
// Usage: bun web/hash-assets.mjs [webDir=web] [outDir=site]
import { createHash } from 'node:crypto';
import { readFileSync, writeFileSync, mkdirSync, copyFileSync } from 'node:fs';
import { join } from 'node:path';

const [webDir = 'web', outDir = 'site'] = process.argv.slice(2);
mkdirSync(outDir, { recursive: true });

const hash = (buf) => createHash('sha256').update(buf).digest('hex').slice(0, 8);
const read = (name) => readFileSync(join(webDir, name));
const emit = (name, data) => (writeFileSync(join(outDir, name), data), name);

// Rewrite references bottom-up so each file's hash covers its already-rewritten dependency names.
// wasm (leaf) -> worker.js (loads wasm) -> app.js (loads wasm + worker) -> index.html (loads app).
const wasm = emit(`sticker_packer_bg.${hash(read('sticker_packer_bg.wasm'))}.wasm`, read('sticker_packer_bg.wasm'));

const workerSrc = read('worker.js').toString().replaceAll('sticker_packer_bg.wasm', wasm);
const worker = emit(`worker.${hash(workerSrc)}.js`, workerSrc);

const appSrc = read('app.js').toString().replaceAll('sticker_packer_bg.wasm', wasm).replaceAll('./worker.js', `./${worker}`);
const app = emit(`app.${hash(appSrc)}.js`, appSrc);

emit('index.html', read('index.html').toString().replaceAll('./app.js', `./${app}`));
copyFileSync(join(webDir, 'favicon.svg'), join(outDir, 'favicon.svg'));
writeFileSync(join(outDir, '.nojekyll'), '');

console.log(`hashed -> ${outDir}: ${app}, ${worker}, ${wasm}`);
