/**
 * Single source of truth for the version → release-tag mapping.
 * Internal versions and release tags on github.com/NitroQ/bea are both X.Y.Z,
 * so the tag is the version with a `v` prefix.
 */
export function releaseTag(version: string): string {
  return `v${version}`;
}
