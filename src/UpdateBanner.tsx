import { useEffect, useReducer } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { initialUpdateState, reduceUpdateState, type UpdateEvent, type UpdateInfo } from "./updateStore";

/**
 * Passive update banner: appears when the Rust side emits `update://available`
 * (from the scheduled GitHub Releases check). Installing is explicit user
 * consent — never automatic.
 */
export function UpdateBanner() {
  const [state, dispatch] = useReducer(
    (state: ReturnType<typeof getInitial>, event: UpdateEvent) => reduceUpdateState(state, event),
    undefined,
    getInitial,
  );

  useEffect(() => {
    const unsubs = [
      listen<UpdateInfo>("update://available", (e) => dispatch({ type: "available", info: e.payload })),
      listen("update://none", () => dispatch({ type: "none" })),
      listen<string>("update://error", (e) => dispatch({ type: "error", message: e.payload })),
    ];
    return () => {
      unsubs.forEach((p) => p.then((unsub) => unsub()));
    };
  }, []);

  if (state.status === "idle" || !state.info) return null;
  const { version, notes } = state.info;

  return (
    <div role="status" className="update-banner" data-version={version}>
      <span>
        Update v{version} available{notes ? ` — ${notes}` : ""}
      </span>
      <button
        type="button"
        disabled={state.status === "installing"}
        onClick={() => {
          dispatch({ type: "install-start" });
          void invoke("install_update_command");
        }}
      >
        {state.status === "installing" ? "Installing…" : "Install & restart"}
      </button>
      <button type="button" onClick={() => dispatch({ type: "dismiss" })}>
        Later
      </button>
    </div>
  );
}

function getInitial() {
  return initialUpdateState;
}
