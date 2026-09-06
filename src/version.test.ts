import { describe, expect, it } from "vitest";
import pkg from "../package.json";
import tauri from "../src-tauri/tauri.conf.json";
import { releaseTag } from "./version";

describe("version consistency", () => {
  it("matches across package.json and tauri.conf.json", () => {
    expect(tauri.version).toBe(pkg.version);
  });
  it("is three-component semver internally (Cargo requires X.Y.Z)", () => {
    expect(pkg.version).toMatch(/^\d+\.\d+\.\d+$/);
  });
});

describe("releaseTag", () => {
  it("maps internal versions to two-digit release tags", () => {
    expect(releaseTag("1.0.0")).toBe("v1.0");
    expect(releaseTag("0.1.0")).toBe("v0.1");
  });
});
