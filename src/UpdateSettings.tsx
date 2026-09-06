import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { initialUpdateState, reduceUpdateState, type UpdateEvent, type UpdateInfo } from "./updateStore";
import Icon from "./Icon";

type CheckState = "idle" | "checking" | "checking-in-browser";

/**
 * Settings section for update status: current version, pending update (if the
 * scheduled check found one), and a manual "Check for updates" button.
 * Shares state shape with the UpdateBanner via updateStore.
 */
export function UpdateSettingsSection() {
  const [state, dispatch] = useState(initialUpdateState);
  const [check, setCheck] = useState<CheckState>("idle");
  const [version, setVersion] = useState<string | null>(null);
  const [lastChecked, setLastChecked] = useState<string | null>(null);

  useEffect(() => {
    getVersion().then(setVersion).catch(() => setVersion(null));
    const unsubs = [
      listen<UpdateInfo>("update://available", (e) => {
        dispatch((current) => reduceUpdateState(current, { type: "available", info: e.payload }));
        setCheck("idle");
        setLastChecked(new Date().toLocaleTimeString());
      }),
      listen("update://none", () => {
        dispatch((current) => reduceUpdateState(current, { type: "none" }));
        setCheck("idle");
        setLastChecked(new Date().toLocaleTimeString());
      }),
      listen<string>("update://error", (e) => {
        dispatch((current) => reduceUpdateState(current, { type: "error", message: e.payload }));
        setCheck("idle");
        setLastChecked(new Date().toLocaleTimeString());
      }),
    ];
    return () => {
      unsubs.forEach((p) => p.then((unsub) => unsub()));
    };
  }, []);

  async function checkForUpdates() {
    setCheck("checking");
    try {
      const result = await invoke<UpdateInfo | null>("check_for_updates_command");
      if (result) {
        dispatch((current) => reduceUpdateState(current, { type: "available", info: result }));
      }
      // `none` / `error` events arrive from the Rust side and reset `check`.
      if (!result) setLastChecked(new Date().toLocaleTimeString());
    } catch {
      // Browser preview has no updater command.
      setCheck("checking-in-browser");
      setLastChecked(new Date().toLocaleTimeString());
    }
  }

  async function install() {
    if (!state.info) return;
    dispatch((current) => reduceUpdateState(current, { type: "install-start" }));
    await invoke("install_update_command").catch((error) => setNotice(`Update failed: ${String(error)}`));
  }

  const statusLine = state.status === "available" && state.info
    ? `Update v${state.info.version} is ready to install.`
    : state.status === "installing"
      ? "Downloading and installing…"
      : lastChecked
        ? `You're up to date (checked ${lastChecked}).`
        : "Checking happens automatically every 6 hours.";

  return (
    <section className="settings-section">
      <div className="settings-section-title">
        <div>
          <h2>Updates</h2>
          <p>Bea keeps itself current from GitHub Releases. Installations always need your confirmation.</p>
        </div>
      </div>
      <div className="settings-row">
        <span className={`setup-status ${state.status === "available" ? "is-ready" : ""}`}>
          <Icon name={state.status === "available" ? "download" : "check"} size={14} />
        </span>
        <span>
          <strong>{version ? `Bea v${version}` : "Bea"}</strong>
          <small>{statusLine}</small>
        </span>
        {state.status === "available" ? (
          <button
            className="secondary small"
            disabled={false}
            onClick={() => void install()}
          >
            Install
          </button>
        ) : (
          <button className="secondary small" disabled={check === "checking"} onClick={() => void checkForUpdates()}>
            {check === "checking" ? "Checking…" : "Check for updates"}
          </button>
        )}
      </div>
    </section>
  );
}

function setNotice(message: string) {
  // Settings has no notice prop; surface failures via the status line instead.
  console.warn(message);
}
