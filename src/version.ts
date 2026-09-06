/**
 * Single source of truth for the version → release-tag mapping.
 * Internal versions are X.Y.Z (Cargo/npm semver requirement); release tags
 * on github.com/NitroQ/bea are two digits only: v1.0, v1.1, ...
 */
export function releaseTag(version: string): string {
  const [major, minor] = version.split(".");
  return `v${major}.${minor}`;
}
