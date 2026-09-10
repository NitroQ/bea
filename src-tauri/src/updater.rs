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
/// exists, `update://none` when up to date, and `update://error` on failure —
/// the Settings listeners rely on all three outcomes to reset their state,
/// so a manual check never leaves the UI stuck on "Checking…".
///
/// Async on purpose: this runs from inside the tokio runtime (scheduled task
/// and Tauri command). The previous `block_on` version panicked at startup
/// with "Cannot start a runtime from within a runtime".
pub async fn check_now(app: &AppHandle) -> Result<Option<UpdateInfo>, String> {
    let updater = match app.updater() {
        Ok(updater) => updater,
        Err(e) => {
            let message = format!("updater unavailable: {e}");
            let _ = app.emit("update://error", &message);
            return Err(message);
        }
    };
    let update = match updater.check().await {
        Ok(update) => update,
        Err(e) => {
            let message = format!("update check failed: {e}");
            let _ = app.emit("update://error", &message);
            return Err(message);
        }
    };
    let Some(update) = update else {
        let _ = app.emit("update://none", ());
        return Ok(None);
    };
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
    Ok(Some(info))
}

/// Downloads and installs the pending update, then restarts the app.
/// Only called from the user-triggered command.
pub async fn download_and_install(app: &AppHandle) -> Result<(), String> {
    let updater = app
        .updater()
        .map_err(|e| format!("updater unavailable: {e}"))?;
    let update = updater
        .check()
        .await
        .map_err(|e| format!("update check failed: {e}"))?
        .ok_or_else(|| "no update available".to_string())?;

    let mut received = 0u64;
    let bytes = update
        .download(
            |chunk: usize, total: Option<u64>| {
                received += chunk as u64;
                let _ = app.emit("update://progress", (received, total));
            },
            || {
                let _ = app.emit("update://downloaded", ());
            },
        )
        .await
        .map_err(|e| format!("update download failed: {e}"))?;

    let _ = app.emit("update://installing", ());
    // Signature was verified by the plugin during download; install + relaunch
    // (on Windows the installer exits the app itself).
    update
        .install(bytes)
        .map_err(|e| format!("update install failed: {e}"))
}

/// Shared scheduler loop, injected with the check future so tests can run it
/// without an app handle. Sleeps out the startup delay first — tokio
/// `interval` fires its first tick immediately, so the delay must be a sleep.
pub async fn run_scheduled_checks<F, Fut>(mut check: F, startup_delay: Duration, check_interval: Duration)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<(), String>>,
{
    tokio::time::sleep(startup_delay).await;
    let mut interval = tokio::time::interval(check_interval);
    loop {
        interval.tick().await;
        if let Err(e) = check().await {
            // Transient (offline, rate limit) — log and retry next tick.
            eprintln!("[updater] scheduled check failed: {e}");
        }
    }
}

/// Background task: one check shortly after startup, then every
/// `CHECK_INTERVAL_HOURS`. Failures are logged and retried next tick.
pub fn spawn_scheduled_checks(app: AppHandle) {
    tauri::async_runtime::spawn(run_scheduled_checks(
        move || {
            let app = app.clone();
            async move { check_now(&app).await.map(|_| ()) }
        },
        Duration::from_secs(STARTUP_CHECK_DELAY_SECS),
        Duration::from_secs(CHECK_INTERVAL_HOURS * 3600),
    ));
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

    // Regression: the scheduled task used tokio interval's immediate first
    // tick + a blocking check inside the async runtime, which panicked at
    // startup ("Cannot start a runtime from within a runtime") and left the
    // app window dead. The scheduler must await (never block_on) and must
    // sleep out the startup delay before its first check.
    mod scheduler {
        use super::super::run_scheduled_checks;
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;
        use std::time::Duration;

        fn counting(checks: Arc<AtomicUsize>) -> impl FnMut() -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>> {
            move || {
                checks.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { Ok(()) })
            }
        }

        #[tokio::test]
        async fn waits_out_the_startup_delay_before_the_first_check() {
            let checks = Arc::new(AtomicUsize::new(0));
            let task = tokio::spawn(run_scheduled_checks(
                counting(checks.clone()),
                Duration::from_millis(150),
                Duration::from_millis(40),
            ));
            tokio::time::sleep(Duration::from_millis(60)).await;
            assert_eq!(
                checks.load(Ordering::SeqCst),
                0,
                "a check ran before the startup delay elapsed"
            );
            task.abort();
        }

        #[tokio::test]
        async fn repeats_checks_after_the_startup_delay() {
            let checks = Arc::new(AtomicUsize::new(0));
            let task = tokio::spawn(run_scheduled_checks(
                counting(checks.clone()),
                Duration::from_millis(30),
                Duration::from_millis(30),
            ));
            tokio::time::sleep(Duration::from_millis(200)).await;
            task.abort();
            assert!(
                checks.load(Ordering::SeqCst) >= 2,
                "checks did not repeat after the startup delay"
            );
        }
    }
}
