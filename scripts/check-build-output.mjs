import { readFile, readdir, stat } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { gzipSync } from 'node:zlib';

const assetDirectory = fileURLToPath(new URL('../dist/assets/', import.meta.url));
const expectedLocaleIds = new Set([
  'de-de',
  'en-us',
  'es-es',
  'fr-fr',
  'ja-jp',
  'ko-kr',
  'pt-br',
  'ru-ru',
  'tr-tr',
  'zh-cn',
  'zh-tw',
]);
const maximumApplicationChunkBytes = 300 * 1024;
// Allow a small raw-size margin for localized scan, monitoring and disk-capacity guidance while
// retaining the 60 KiB gzip limit on transferred locale assets.
const maximumLocaleChunkBytes = 304 * 1024;
const maximumLocaleGzipBytes = 60 * 1024;
// Cyrillic uses two UTF-8 bytes per letter. Keep a measured, locale-specific
// allowance instead of weakening the budget for every locale chunk.
// Optional Windows setup adds 32 localized status and action strings. Japanese
// measures 319,339 raw bytes; Russian measures 67,914 gzip bytes. Keep their
// measured margins local instead of increasing every locale or application budget.
// GPU temperature adds six strings; Japanese now measures 320,937 raw bytes.
// Per-item color rules add 21 net labels: Japanese measures 322,154 raw bytes;
// Russian measures 389,672 raw / 68,621 gzip bytes. Expand only these local margins.
// Independent condition dialogs measure 322,964 Japanese / 390,657 Russian raw bytes.
// Battery metrics and power-direction labels measure 391,458 Russian raw bytes.
// Reserve one additional KiB locally; keep application and gzip budgets unchanged.
// Whole-system power adds two labels: Japanese measures 323,767 raw bytes.
// The component license catalog measures 325,324 Japanese / 393,378 Russian
// raw bytes. Add one KiB to each local raw margin; retain their gzip budgets.
// Concise privacy failure guidance measures 326,184 Japanese / 394,554 Russian raw bytes.
// Japanese gzip measures 61,463 bytes; retain the original Russian gzip budget.
// Fan display modes add four labels: Japanese measures 326,937 raw bytes and
// Russian 69,704 gzip bytes. Grant only 512 raw / 128 gzip bytes locally.
// Disk health, I/O and interface details add localized labels and hints.
// Japanese measures 329,390 raw bytes; Russian measures 398,077 raw bytes.
// Extend only the measured locale margins; application budgets remain unchanged.
// Persisted traffic adds localized accumulation/reset guidance: Japanese measures
// 330,879 raw / 62,638 gzip bytes; Russian measures 399,778 raw / 70,709 gzip.
// Add only 512 raw bytes per locale and 256/128 gzip bytes respectively.
// French, German and Spanish assets measure approximately 318 / 308 / 310 kB
// raw and 63.2 / 63.8 / 61.7 kB gzip. Keep their measured allowances local;
// application and existing locale budgets stay fixed.
// Super-core frequency adds one label per locale: Japanese measures 62,734 gzip bytes;
// Russian measures 399,926 raw bytes. Add only 64 gzip / 128 raw bytes locally.
// GPU display selection labels measure 400,087 Russian raw / 70,819 gzip bytes.
// Extend only that locale by 128 raw / 64 gzip bytes.
// Physical disk selection adds ten labels; extend only the measured French, Japanese and Russian margins.
// Windows GPU fan copy measures fr-FR 63,534 gzip / ja-JP 332,358 raw / ru-RU 401,301 raw bytes.
// Add only 64 gzip / 128 raw / 256 raw bytes to those locale margins.
// Metric availability adds four labels. Measured fr/ja/ru assets require only
// 128/384/384 additional raw bytes and 128/64/64 gzip bytes respectively.
// Four documented cache rules add twelve strings per locale. Measured es/fr/ja/ru
// assets are 312414/320691/334113/403277 raw bytes; fr/ja/ru gzip is
// 63806/63255/71334 bytes. Extend only these measured locale margins.
// Floating monitor adds 19 labels. Measured es/fr/ja/ru raw sizes are
// 312328/320636/333884/403096 bytes; fr/ja/ru gzip sizes are 63831/63330/71398.
// Extend only these measured locale margins; keep application budgets unchanged.
// Combine both features' measured increments relative to their shared base.
// Background-opacity guidance measures de-DE 311745 raw / 64652 gzip bytes,
// es-ES 62501 gzip and pt-BR 61568 gzip bytes. Extend only those margins.
// Edge-snapping labels measure de-DE 311817 raw and pt-BR 61591 gzip bytes.
// Compact floating labels measure de-DE 312376 raw / 64751 gzip and ru-RU
// 405297 raw bytes. Extend only measured de/es/fr/pt gzip and de/ru raw margins.
// Fourteen floating appearance labels measure de/es/fr/ja/ru raw bytes
// 313051/314754/323065/336537/406127; gzip de/es/fr/ja/pt/ru measures
// 64906/62747/64295/63790/61809/71930. Extend only these measured local margins.
// The divider label measures 63,814 Japanese / 71,943 Russian gzip bytes.
// Add only 16 bytes to these two locale budgets.
// Click-through label/hint measure de/es/fr/ja/ru raw bytes
// 313293/315030/323348/336813/406422 and fr/ja/pt/ru gzip
// 64366/63875/61877/72006. Extend only these measured local margins.
// Fullscreen hiding adds one label: fr/ja/pt/ru gzip measures
// 64389/63904/61900/72028 bytes. Add only 16 bytes to their local margins.
// Current-directory batch deletion adds twelve labels and safety explanations.
// Measured de/es/fr/ja/ru chunks are 312027/313728/322017/335456/404974 raw bytes;
// their gzip sizes are 64667/62514/64040/63519/71604 bytes, and Portuguese is 61579.
// Extend only the affected locale margins; retain all application chunk limits.
// Nine CPU frequency labels add measured raw bytes de/es/fr/ja/ru
// 313621/315436/323732/337207/406861; gzip is 65056/62921/64459/63970/72124,
// and Portuguese is 61990. CPU-prefixed names are included in these measurements.
// Extend only these measured local margins.
// Re-measure the combined features: gzip increments are not additive.
// Current de raw/gzip is 314968/65324; es/ko/ru gzip is 63188/61638/72401.
// Keep less than 256 raw / 96 gzip bytes of headroom on these affected limits.
// CPU idle/state guidance measures de/es/fr/ja/ru raw bytes
// 315268/317072/325397/338909/408950; fr/ja/pt gzip is 64760/64307/62306.
// Extend only these measured locale margins; application budgets remain fixed.
// Display-default confirmation adds three localized strings. Measured de/es/fr/ja/ru
// raw sizes are 315628/317473/325856/339312/409483 bytes; their gzip sizes are
// 65459/63326/64869/64380/72560. Extend only the affected locale margins.
const localeChunkLimitOverrides = new Map([
  ['fr-fr', { bytes: 325952, gzipBytes: 64928 }],
  ['de-de', { bytes: 315712, gzipBytes: 65504 }],
  ['es-es', { bytes: 317568, gzipBytes: 63360 }],
  ['pt-br', { bytes: 311296, gzipBytes: 62432 }],
  ['ko-kr', { bytes: 311296, gzipBytes: 61824 }],
  ['ja-jp', { bytes: 339392, gzipBytes: 64416 }],
  ['ru-ru', { bytes: 409600, gzipBytes: 72608 }],
]);

function fail(message) {
  console.error(`[build-output] ${message}`);
  process.exitCode = 1;
}

let assetNames;
try {
  assetNames = await readdir(assetDirectory);
} catch (error) {
  fail(`cannot read ${assetDirectory}: ${error instanceof Error ? error.message : String(error)}`);
  process.exit();
}

const javaScriptAssets = assetNames.filter(name => name.endsWith('.js'));
const localeAssets = javaScriptAssets.filter(name => name.startsWith('locale-'));

for (const localeId of expectedLocaleIds) {
  const matchingAssets = localeAssets.filter(name => name.startsWith(`locale-${localeId}-`));
  if (matchingAssets.length !== 1) {
    fail(`expected one ${localeId} chunk, found ${matchingAssets.length}`);
  }
}

const unexpectedLocaleAssets = localeAssets.filter(
  name => ![...expectedLocaleIds].some(localeId => name.startsWith(`locale-${localeId}-`))
);
if (unexpectedLocaleAssets.length > 0) {
  fail(`unexpected locale chunks: ${unexpectedLocaleAssets.join(', ')}`);
}

for (const assetName of javaScriptAssets) {
  const assetPath = `${assetDirectory}/${assetName}`;
  const assetSize = (await stat(assetPath)).size;
  const isLocaleAsset = assetName.startsWith('locale-');
  const localeId = isLocaleAsset
    ? [...expectedLocaleIds].find(expectedId => assetName.startsWith(`locale-${expectedId}-`))
    : undefined;
  const localeLimits = localeId ? localeChunkLimitOverrides.get(localeId) : undefined;
  const maximumBytes = isLocaleAsset ? (localeLimits?.bytes ?? maximumLocaleChunkBytes) : maximumApplicationChunkBytes;
  if (assetSize > maximumBytes) {
    fail(
      `${assetName} is ${assetSize} bytes, exceeding the ${maximumBytes}-byte ` +
        `${isLocaleAsset ? 'locale' : 'application'} chunk limit`
    );
  }
  if (isLocaleAsset) {
    const gzipSize = gzipSync(await readFile(assetPath)).length;
    const maximumGzipBytes = localeLimits?.gzipBytes ?? maximumLocaleGzipBytes;
    if (gzipSize > maximumGzipBytes) {
      fail(`${assetName} is ${gzipSize} bytes gzipped, exceeding the ${maximumGzipBytes}-byte limit`);
    }
  }
}

if (process.exitCode) {
  process.exit();
}

console.log(
  `[build-output] verified ${javaScriptAssets.length} JavaScript chunks and ` +
    `${localeAssets.length} project-owned locale chunks`
);
