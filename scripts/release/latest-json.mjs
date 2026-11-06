// Build the updater manifest (`latest.json`) from the signed release files.
//
//   node scripts/release/latest-json.mjs <dist-dir> <version> <repo> <tag> > latest.json
//
// Expects the files the release jobs renamed to space-free names:
//   rl-companion_<v>_x64-setup.exe(.sig)       -> windows-x86_64 (+ -nsis)
//   rl-companion_<v>_aarch64.app.tar.gz(.sig)  -> darwin-aarch64 (+ -app)
// Each `.sig` holds the base64 signature tauri's bundler wrote; the manifest
// carries it inline. Both platforms are required: a manifest missing one
// would leave that platform's copies silently never updating.
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

export const PLATFORMS = [
  { keys: ["windows-x86_64", "windows-x86_64-nsis"], file: (v) => `rl-companion_${v}_x64-setup.exe` },
  { keys: ["darwin-aarch64", "darwin-aarch64-app"], file: (v) => `rl-companion_${v}_aarch64.app.tar.gz` },
];

/** `files` maps a file name to its contents (only `.sig` contents are read). */
export function manifest({ version, repo, tag, files, notes, pubDate }) {
  const platforms = {};
  for (const p of PLATFORMS) {
    const name = p.file(version);
    if (!(name in files)) throw new Error(`missing ${name}`);
    const sig = files[`${name}.sig`];
    if (sig === undefined) throw new Error(`missing ${name}.sig (was the build signed?)`);
    const entry = {
      signature: sig.trim(),
      url: `https://github.com/${repo}/releases/download/${tag}/${encodeURIComponent(name)}`,
    };
    for (const k of p.keys) platforms[k] = entry;
  }
  return { version, notes, pub_date: pubDate, platforms };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [dir, version, repo, tag] = process.argv.slice(2);
  if (!dir || !version || !repo || !tag) throw new Error("usage: latest-json.mjs <dir> <version> <repo> <tag>");
  const files = {};
  for (const f of readdirSync(dir)) files[f] = f.endsWith(".sig") ? readFileSync(join(dir, f), "utf8") : "";
  const notes = `RL Companion ${version}. See https://github.com/${repo}/releases/tag/${tag}`;
  const pubDate = new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
  console.log(JSON.stringify(manifest({ version, repo, tag, files, notes, pubDate }), null, 2));
}
