//! Update checks against GitHub Releases (github.com/NitroQ/bea).
//!
//! Security model:
//! - Bundles are minisign-verified by tauri-plugin-updater against the pubkey
//!   embedded in tauri.conf.json; unsigned/tampered downloads are rejected.
//! - The endpoint is pinned (HTTPS, GitHub only); the manifest (`latest.json`)
//!   is published by CI, not scraped.
//! - Scheduled checks are passive; installs happen only on user consent.

use serde::Serialize;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;

/// How often the background task re-checks GitHub Releases for updates.
pub const CHECK_INTERVAL_HOURS: u64 = 6;

/// Seconds after startup before the first (startup) check runs.
pub const STARTUP_CHECK_DELAY_SECS: u64 = 30;

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub version: String,
    pub notes: String,
    pub current_version: String,
}

/// Checks GitHub Releases once. Emits `update://available` when an update
/// exists; returns `Some(UpdateInfo)` in that case, `None` when up to date.
pub fn check_now(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = app
        .updater()
        .map_err(|e| format!("updater unavailable: {e}"))?;
    let update = tauri::async_runtime::block_on(updater.check())
        .map_err(|e| format!("update check failed: {e}"))?;
    Ok(update.map(|update| {
        let info = UpdateInfo {
            version: update.version.clone(),
            notes: update
                .body
                .clone()
                .unwrap_or_else(|| format!("Update to {}", update.version)),
            current_version: app
                .config()
                .version
                .clone()
                .unwrap_or_else(|| "unknown".into()),
        };
        let _ = app.emit("update://available", &info);
        info
    }))
}

/// Downloads and installs the pending update, then restarts the app.
/// Only called from the user-triggered command.
pub fn download_and_install(app: &AppHandle) -> Result<(), String> {
    let updater = app
        .updater()
        .map_err(|e| format!("updater unavailable: {e}"))?;
    let update = tauri::async_runtime::block_on(updater.check())
        .map_err(|e| format!("update check failed: {e}"))?
        .ok_or_else(|| "no update available".to_string())?;

    let mut received = 0u64;
    let bytes = tauri::async_runtime::block_on(update.download(
        |chunk: usize, total: Option<u64>| {
            received += chunk as u64;
            let _ = app.emit("update://progress", (received, total));
        },
        || {
            let _ = app.emit("update://downloaded", ());
        },
    ))
    .map_err(|e| format!("update download failed: {e}"))?;

    let _ = app.emit("update://installing", ());
    // Signature was verified by the plugin during download; install + relaunch
    // (on Windows the installer exits the app itself).
    update
        .install(bytes)
        .map_err(|e| format!("update install failed: {e}"))
}

/// Background task: one check shortly after startup, then every
/// `CHECK_INTERVAL_HOURS`. Failures are logged and retried next tick.
pub fn spawn_scheduled_checks(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(STARTUP_CHECK_DELAY_SECS)).await;
        let mut interval = tokio::time::interval(Duration::from_secs(CHECK_INTERVAL_HOURS * 3600));
        loop {
            interval.tick().await;
            if let Err(e) = check_now(&app) {
                // Transient (offline, rate limit) — log and retry next tick.
                eprintln!("[updater] scheduled check failed: {e}");
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interval_is_at_least_one_hour() {
        assert!(CHECK_INTERVAL_HOURS >= 1);
    }

    #[test]
    fn startup_delay_is_bounded() {
        assert!(STARTUP_CHECK_DELAY_SECS < 120);
    }
}
