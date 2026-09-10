import { describe, expect, it } from "vitest";
import { reduceUpdateState, initialUpdateState, type UpdateState, type UpdateInfo } from "./updateStore";

describe("updateStore", () => {
  it("starts hidden with no version and no error", () => {
    expect(initialUpdateState).toEqual({ status: "idle", info: null, message: null });
  });

  it("shows the banner when an update is available", () => {
    const info: UpdateInfo = { version: "1.1.0", notes: "Fixes", current_version: "1.0.0" };
    const next = reduceUpdateState(initialUpdateState, { type: "available", info });
    expect(next.status).toBe("available");
    expect(next.info?.version).toBe("1.1.0");
  });

  it("stays hidden on update://none and update://error", () => {
    expect(reduceUpdateState(initialUpdateState, { type: "none" }).status).toBe("idle");
    expect(reduceUpdateState(initialUpdateState, { type: "error", message: "offline" }).status).toBe("idle");
  });

  it("keeps the error text so the settings status line can show it", () => {
    // A failed manual check must not vanish silently — the user pressed the
    // button and deserves to know what happened.
    const errored = reduceUpdateState(initialUpdateState, { type: "error", message: "offline" });
    expect(errored.message).toBe("offline");
    // A later "none" clears the stale error.
    expect(reduceUpdateState(errored, { type: "none" }).message).toBeNull();
    // A fresh available update also clears it.
    const info: UpdateInfo = { version: "1.1.0", notes: "Fixes", current_version: "1.0.0" };
    expect(reduceUpdateState(errored, { type: "available", info }).message).toBeNull();
  });

  it("marks installing only from the available state", () => {
    const info: UpdateInfo = { version: "1.1.0", notes: "Fixes", current_version: "1.0.0" };
    const available = reduceUpdateState(initialUpdateState, { type: "available", info });
    expect(reduceUpdateState(available, { type: "install-start" }).status).toBe("installing");
    // Dismissing clears the banner entirely
    expect(reduceUpdateState(available, { type: "dismiss" }).status).toBe("idle");
    // install-start from idle is ignored
    expect(reduceUpdateState(initialUpdateState, { type: "install-start" }).status).toBe("idle");
  });

  it("types: state is a discriminated union", () => {
    const state: UpdateState = initialUpdateState;
    expect(state.status).toBe("idle");
  });
});
