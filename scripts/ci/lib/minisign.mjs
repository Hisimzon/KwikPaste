// Verifies the updater signatures (`.sig`) the way the 1.x updater and kwikpaste-updater do.
//
// A `.sig` file is base64 of a minisign signature file; the public key in tauri.conf.json (and
// crates/kwikpaste-updater/pubkey.txt) is base64 of a minisign public key file. Signatures made by
// `tauri signer` are prehashed (`ED`: Ed25519 over BLAKE2b-512 of the file); legacy `Ed` signs the bytes.
// The global signature covers the signature plus the trusted comment.
import { createHash, createPublicKey, verify } from "node:crypto";

const decodeBase64Text = (text) => {
  return Buffer.from(text.trim(), "base64").toString("utf8");
};

/**
 * Parses a public key in the tauri.conf.json format: `{ keyId, key }` (key id as upper-case hex, the way
 * `minisign` prints it).
 */
export const parsePublicKey = (text) => {
  const lines = decodeBase64Text(text).split(/\r?\n/);
  const raw = Buffer.from(lines[1].trim(), "base64");
  if (raw.length !== 42 || raw.toString("latin1", 0, 2) !== "Ed") {
    throw new Error("not a minisign Ed25519 public key");
  }
  const key = createPublicKey({
    format: "jwk",
    key: {
      crv: "Ed25519",
      kty: "OKP",
      x: raw.subarray(10).toString("base64url"),
    },
  });

  return {
    key,
    keyId: Buffer.from(raw.subarray(2, 10))
      .reverse()
      .toString("hex")
      .toUpperCase(),
    rawKeyId: raw.subarray(2, 10),
  };
};

/**
 * Verifies `data` against a `.sig` file's contents. Returns the trusted comment; throws when invalid.
 */
export const verifySignature = (data, sigText, publicKey) => {
  const lines = decodeBase64Text(sigText).split(/\r?\n/);
  const signature = Buffer.from(lines[1].trim(), "base64");
  const trustedLine = lines[2];
  const globalSignature = Buffer.from(lines[3].trim(), "base64");
  if (signature.length !== 74 || !trustedLine.startsWith("trusted comment: ")) {
    throw new Error("malformed minisign signature");
  }

  const algorithm = signature.toString("latin1", 0, 2);
  if (!signature.subarray(2, 10).equals(publicKey.rawKeyId)) {
    throw new Error(`signed by another key than ${publicKey.keyId}`);
  }
  const message =
    algorithm === "ED" ? createHash("blake2b512").update(data).digest() : data;
  if (algorithm !== "ED" && algorithm !== "Ed") {
    throw new Error(`unknown signature algorithm ${algorithm}`);
  }
  if (!verify(null, message, publicKey.key, signature.subarray(10))) {
    throw new Error("signature does not match the file");
  }

  const trusted = trustedLine.slice("trusted comment: ".length);
  const global = Buffer.concat([
    signature.subarray(10),
    Buffer.from(trusted, "utf8"),
  ]);
  if (!verify(null, global, publicKey.key, globalSignature)) {
    throw new Error("global signature does not match the trusted comment");
  }

  return trusted;
};
