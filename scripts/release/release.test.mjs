// node --test scripts/release/
import assert from "node:assert/strict";
import { test } from "node:test";
import { manifest } from "./latest-json.mjs";
import { plan, tauriOverlay } from "./release-plan.mjs";
import crypto from "node:crypto";
import { parsePubkey, verify } from "./verify-sigs.mjs";

test("a test tag publishes a signed build to the test channel only", () => {
  const p = plan("refs/tags/v0.1.0-test.6", "41", true);
  assert.deepEqual(p, { version: "0.1.0-test.6", sign: true, publish: true, channels: ["test"] });
  assert.deepEqual(tauriOverlay(p), { version: "0.1.0-test.6", bundle: { createUpdaterArtifacts: true } });
});

test("a plain tag feeds both channels", () => {
  assert.deepEqual(plan("refs/tags/v0.2.0", "1", true).channels, ["stable", "test"]);
});

test("a tag without the signing key fails instead of shipping an unupdatable build", () => {
  assert.throws(() => plan("refs/tags/v0.1.0-test.6", "1", false), /must be signed/);
});

test("a tag that is not v<semver> is refused", () => {
  for (const t of ["0.1.0", "v0.1", "v01.0.0", "vnext", "updates-test"]) {
    assert.throws(() => plan(`refs/tags/${t}`, "1", true), /not v<semver>/, t);
  }
});

test("a branch push is an unpublished dry run, signed only when the key exists", () => {
  assert.deepEqual(plan("refs/heads/feat/x", "7", false), {
    version: "0.0.0-dryrun.7", sign: false, publish: false, channels: [],
  });
  assert.equal(plan("refs/heads/feat/x", "7", true).sign, true);
  assert.deepEqual(tauriOverlay(plan("refs/heads/x", "7", false)).bundle, { createUpdaterArtifacts: false });
});

const files = (v) => ({
  [`rl-companion_${v}_x64-setup.exe`]: "",
  [`rl-companion_${v}_x64-setup.exe.sig`]: "V0lOU0lH\n",
  [`rl-companion_${v}_aarch64.app.tar.gz`]: "",
  [`rl-companion_${v}_aarch64.app.tar.gz.sig`]: "TUFDU0lH",
  [`rl-companion_${v}_aarch64.dmg`]: "",
});

test("latest.json points each platform at the tag's asset with its signature", () => {
  const m = manifest({
    version: "0.1.0-test.6", repo: "Raid-Ledger-Plugins/rl-companion", tag: "v0.1.0-test.6",
    files: files("0.1.0-test.6"), notes: "n", pubDate: "2026-11-05T16:00:00Z",
  });
  assert.equal(m.version, "0.1.0-test.6");
  assert.equal(m.pub_date, "2026-11-05T16:00:00Z");
  const base = "https://github.com/Raid-Ledger-Plugins/rl-companion/releases/download/v0.1.0-test.6";
  assert.deepEqual(m.platforms["windows-x86_64"], {
    signature: "V0lOU0lH", url: `${base}/rl-companion_0.1.0-test.6_x64-setup.exe`,
  });
  assert.deepEqual(m.platforms["windows-x86_64-nsis"], m.platforms["windows-x86_64"]);
  assert.equal(m.platforms["darwin-aarch64"].url, `${base}/rl-companion_0.1.0-test.6_aarch64.app.tar.gz`);
  assert.equal(m.platforms["darwin-aarch64"].signature, "TUFDU0lH");
  assert.deepEqual(Object.keys(m.platforms).sort(), ["darwin-aarch64", "darwin-aarch64-app", "windows-x86_64", "windows-x86_64-nsis"]);
});

test("latest.json refuses a release missing a platform or a signature", () => {
  const args = { version: "1.0.0", repo: "r/r", tag: "v1.0.0", notes: "", pubDate: "" };
  const noSig = files("1.0.0");
  delete noSig["rl-companion_1.0.0_x64-setup.exe.sig"];
  assert.throws(() => manifest({ ...args, files: noSig }), /x64-setup\.exe\.sig/);
  const noMac = files("1.0.0");
  delete noMac["rl-companion_1.0.0_aarch64.app.tar.gz"];
  assert.throws(() => manifest({ ...args, files: noMac }), /aarch64\.app\.tar\.gz/);
});

// A throwaway minisign keypair in tauri's encoding (base64 of the text files).
function keypair(id = Buffer.from("0123456789abcdef", "hex")) {
  const { publicKey, privateKey } = crypto.generateKeyPairSync("ed25519");
  const raw = Buffer.from(publicKey.export({ format: "jwk" }).x, "base64url");
  const pubFile = `untrusted comment: minisign public key\n${Buffer.concat([Buffer.from("Ed"), id, raw]).toString("base64")}\n`;
  const sign = (data, prehash = true) => {
    const msg = prehash ? crypto.createHash("blake2b512").update(data).digest() : data;
    const sig = crypto.sign(null, msg, privateKey);
    const trusted = "timestamp:1793903486\tfile:x";
    const global = crypto.sign(null, Buffer.concat([sig, Buffer.from(trusted)]), privateKey);
    const text = `untrusted comment: signature from tauri secret key\n${Buffer.concat([Buffer.from(prehash ? "ED" : "Ed"), id, sig]).toString("base64")}\ntrusted comment: ${trusted}\n${global.toString("base64")}\n`;
    return Buffer.from(text).toString("base64");
  };
  return { pubkey: Buffer.from(pubFile).toString("base64"), sign };
}

test("a release signature made with the app's key verifies (prehashed and raw)", () => {
  const k = keypair();
  const pub = parsePubkey(k.pubkey);
  verify(pub, Buffer.from("installer bytes"), k.sign(Buffer.from("installer bytes")));
  verify(pub, Buffer.from("installer bytes"), k.sign(Buffer.from("installer bytes"), false));
});

test("a signature over other bytes, or by another key, is refused", () => {
  const k = keypair();
  const pub = parsePubkey(k.pubkey);
  assert.throws(() => verify(pub, Buffer.from("tampered"), k.sign(Buffer.from("original"))), /does not match/);
  const other = keypair(Buffer.from("fedcba9876543210", "hex"));
  assert.throws(() => verify(pub, Buffer.from("x"), other.sign(Buffer.from("x"))), /different key/);
  const sameId = keypair();
  assert.throws(() => verify(pub, Buffer.from("x"), sameId.sign(Buffer.from("x"))), /does not match/);
});

test("the app's committed public key parses", async () => {
  const { readFileSync } = await import("node:fs");
  const conf = JSON.parse(readFileSync(new URL("../../apps/companion/src-tauri/tauri.conf.json", import.meta.url), "utf8"));
  assert.equal(parsePubkey(conf.plugins.updater.pubkey).id.length, 8);
});
