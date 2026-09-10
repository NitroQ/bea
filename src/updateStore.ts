export interface UpdateInfo {
  version: string;
  notes: string;
  current_version: string;
}

export type UpdateState = {
  status: "idle" | "available" | "installing";
  info: UpdateInfo | null;
  /// Why the last check failed (network, GitHub unreachable…). The Settings
  /// status line renders it so a manual check never fails silently.
  message: string | null;
};

export const initialUpdateState: UpdateState = { status: "idle", info: null, message: null };

export type UpdateEvent =
  | { type: "available"; info: UpdateInfo }
  | { type: "none" }
  | { type: "error"; message: string }
  | { type: "install-start" }
  | { type: "dismiss" };

/**
 * Pure reducer for update-banner state. The React component subscribes to
 * Tauri events and feeds them through this; installs are user-triggered.
 */
export function reduceUpdateState(state: UpdateState, event: UpdateEvent): UpdateState {
  switch (event.type) {
    case "available":
      return { status: "available", info: event.info, message: null };
    case "none":
      // A definitive "no updates" also clears any earlier error message.
      return { status: "idle", info: null, message: null };
    case "error":
      return { status: "idle", info: null, message: event.message };
    case "install-start":
      return state.status === "available" ? { ...state, status: "installing" } : state;
    case "dismiss":
      return { status: "idle", info: null, message: null };
  }
}
