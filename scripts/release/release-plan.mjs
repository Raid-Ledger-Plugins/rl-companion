// Decide what a release.yml run builds (in-app updates).
//
//   node scripts/release/release-plan.mjs <GITHUB_REF> <RUN_NUMBER> <HAS_KEY:true|false> <CONF_OUT>
//
// Writes the `tauri build --config` overlay to CONF_OUT and prints
// `version=…`, `sign=…`, `publish=…`, `channels=…` lines for $GITHUB_OUTPUT.
// A `v*` tag publishes and MUST sign (no key = hard failure, so a release can
// never ship that installed copies can't update to). Any other ref is a dry
// run: version 0.0.0-dryrun.N, signed only when the key is available.
import { writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const SEMVER = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/;

export function plan(ref, runNumber, hasKey) {
  const tag = ref.startsWith("refs/tags/") ? ref.slice("refs/tags/".length) : null;
  if (tag !== null) {
    if (!tag.startsWith("v") || !SEMVER.test(tag.slice(1))) {
      throw new Error(`tag ${tag} is not v<semver> (e.g. v0.1.0-test.6 or v0.2.0)`);
    }
    if (!hasKey) {
      throw new Error("TAURI_SIGNING_PRIVATE_KEY is not set: a tagged release must be signed");
    }
    const version = tag.slice(1);
    const test = version.includes("-");
    // A plain release feeds both channels; a test build only the test channel.
    return { version, sign: true, publish: true, channels: test ? ["test"] : ["stable", "test"] };
  }
  return { version: `0.0.0-dryrun.${runNumber}`, sign: hasKey, publish: false, channels: [] };
}

export function tauriOverlay(p) {
  return { version: p.version, bundle: { createUpdaterArtifacts: p.sign } };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [ref = "", run = "0", hasKey = "false", out = ""] = process.argv.slice(2);
  if (!out) throw new Error("usage: release-plan.mjs <ref> <run> <hasKey> <confOut>");
  const p = plan(ref, run, hasKey === "true");
  writeFileSync(out, JSON.stringify(tauriOverlay(p), null, 2) + "\n");
  console.log(`version=${p.version}\nsign=${p.sign}\npublish=${p.publish}\nchannels=${p.channels.join(" ")}`);
}
