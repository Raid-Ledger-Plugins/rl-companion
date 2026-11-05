// Check every updater signature in a release against the public key the app
// trusts (tauri.conf.json plugins.updater.pubkey), so a release can never
// publish an update its installed copies would refuse.
//
//   node scripts/release/verify-sigs.mjs <tauri.conf.json> <dir>
//
// tauri's pubkey and .sig files are base64 of minisign text files. A
// signature is "ED" (BLAKE2b-512 prehash) or "Ed" (raw) + key id + 64 bytes,
// followed by a trusted comment signed together with the signature.
import crypto from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const b64 = (s) => Buffer.from(s.trim(), "base64");

/** `{ id, key }` from tauri's base64 pubkey. */
export function parsePubkey(pubkey) {
  const line = b64(pubkey).toString("utf8").split("\n")[1];
  if (!line) throw new Error("pubkey has no key line");
  const raw = b64(line);
  if (raw.length !== 42 || raw.subarray(0, 2).toString() !== "Ed") throw new Error("pubkey is not an Ed25519 minisign key");
  const key = crypto.createPublicKey({
    key: { kty: "OKP", crv: "Ed25519", x: raw.subarray(10).toString("base64url") },
    format: "jwk",
  });
  return { id: raw.subarray(2, 10), key };
}

/** Throws unless `sigB64` signs `data` with `pub`. */
export function verify(pub, data, sigB64) {
  const lines = b64(sigB64).toString("utf8").split("\n");
  const sig = b64(lines[1] ?? "");
  const trusted = (lines[2] ?? "").replace(/^trusted comment: /, "");
  const globalSig = b64(lines[3] ?? "");
  if (sig.length !== 74) throw new Error("malformed signature");
  if (!sig.subarray(2, 10).equals(pub.id)) throw new Error("signed by a different key");
  const alg = sig.subarray(0, 2).toString();
  const msg = alg === "ED" ? crypto.createHash("blake2b512").update(data).digest() : data;
  if (!crypto.verify(null, msg, pub.key, sig.subarray(10))) throw new Error("signature does not match the file");
  if (!crypto.verify(null, Buffer.concat([sig.subarray(10), Buffer.from(trusted)]), pub.key, globalSig)) {
    throw new Error("trusted comment signature is invalid");
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const [conf, dir] = process.argv.slice(2);
  if (!conf || !dir) throw new Error("usage: verify-sigs.mjs <tauri.conf.json> <dir>");
  const pub = parsePubkey(JSON.parse(readFileSync(conf, "utf8")).plugins.updater.pubkey);
  const sigs = readdirSync(dir).filter((f) => f.endsWith(".sig"));
  if (sigs.length === 0) throw new Error(`no .sig files in ${dir}`);
  for (const s of sigs) {
    verify(pub, readFileSync(join(dir, s.slice(0, -4))), readFileSync(join(dir, s), "utf8"));
    console.log(`ok  ${s}`);
  }
}
