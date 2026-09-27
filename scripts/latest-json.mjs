/**
 * Generates the Tauri updater manifest (latest.json) for the current release.
 *
 * Usage: node scripts/latest-json.mjs <tag> [notes]
 *   e.g. node scripts/latest-json.mjs v1.0.3 "Hotfix for the updater"
 *
 * Reads the .sig files produced by `tauri build` and publishes a manifest
 * pointing at the release-download URLs for the NSIS setup exe. The tag is the
 * full internal version (X.Y.Z), so the version in the manifest is the tag
 * without its leading `v`. Invoked by .github/workflows/release.yml on tag push.
 */
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [tag, notes = `Release ${tag}`] = process.argv.slice(2);
if (!/^v\d+\.\d+\.\d+$/.test(tag ?? "")) {
  console.error(`Usage: node scripts/latest-json.mjs <vX.Y.Z tag> [notes] — got "${tag}"`);
  process.exit(1);
}

const version = tag.slice(1); // v1.0.3 -> 1.0.3 (updater needs full semver)
const repo = "NitroQ/bea";
const base = `https://github.com/${repo}/releases/download/${tag}`;

// `tauri build --target <triple>` nests output under target/<triple>/; a
// triple-less build lands in target/release directly. Both can exist locally
// with artifacts from different releases, so the winner is whichever one
// actually holds this tag's signature.
const candidates = [
  "src-tauri/target/release/bundle/nsis",
  "src-tauri/target/x86_64-pc-windows-msvc/release/bundle/nsis",
];

// A bundle folder also accumulates artifacts from every previous build, so the
// tag's own signature has to be selected explicitly — taking the first match
// would publish an installer from an older release.
let bundleDir = null;
let sigPath = null;
for (const dir of candidates) {
  if (!existsSync(dir)) continue;
  const found = readdirSync(dir).find(
    (f) => f.endsWith(".exe.sig") && f.includes(version),
  );
  if (found) {
    bundleDir = dir;
    sigPath = found;
    break;
  }
}
if (!sigPath) {
  throw new Error(
    `No .exe.sig for ${version} in ${candidates.join(" or ")} — did tauri build run with TAURI_SIGNING_PRIVATE_KEY?`,
  );
}
const setupExe = sigPath.replace(/\.sig$/, "");
const signature = readFileSync(join(bundleDir, sigPath), "utf8").trim();

const manifest = {
  version,
  notes,
  pub_date: new Date().toISOString(),
  platforms: {
    "windows-x86_64": {
      signature,
      url: `${base}/${setupExe}`,
    },
  },
};

writeFileSync("latest.json", JSON.stringify(manifest, null, 2) + "\n");
console.log(`latest.json written for ${version} -> ${base}/${setupExe}`);
