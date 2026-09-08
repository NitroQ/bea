/**
 * Generates the Tauri updater manifest (latest.json) for the current release.
 *
 * Usage: node scripts/latest-json.mjs <tag> <notes>
 *   e.g. node scripts/latest-json.mjs v1.0 "First stable release"
 *
 * Reads the .sig files produced by `tauri build` and publishes a manifest
 * pointing at the release-download URLs for the NSIS setup exe. The tag maps
 * to the internal X.Y.Z version as X.Y.0 (release tags are two digits).
 * Invoked by .github/workflows/release.yml on tag push.
 */
import { readFileSync, readdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [tag, notes = `Release ${tag}`] = process.argv.slice(2);
if (!/^v\d+\.\d+$/.test(tag ?? "")) {
  console.error(`Usage: node scripts/latest-json.mjs <vX.Y tag> [notes] — got "${tag}"`);
  process.exit(1);
}

const version = `${tag.slice(1)}.0`; // v1.0 -> 1.0.0 (updater needs full semver)
const repo = "NitroQ/bea";
const bundleDir = "src-tauri/target/release/bundle/nsis";
const base = `https://github.com/${repo}/releases/download/${tag}`;

const sigPath = readdirSync(bundleDir).find((f) => f.endsWith(".exe.sig"));
if (!sigPath) throw new Error(`No .exe.sig in ${bundleDir} — did tauri build run with TAURI_SIGNING_PRIVATE_KEY?`);
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
