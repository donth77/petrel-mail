//! Signed updates, asked for rather than arriving.
//!
//! The check is manual on purpose. An updater that phones home at launch is
//! a second network dependency between a person and their mail, and one
//! that can replace the running program: worth having, not worth doing
//! behind someone's back. A button in Settings asks; nothing else does.
//!
//! What makes it safe to install anything at all is the signature. The
//! public key is compiled into the app from `tauri.conf.json`; the private
//! key lives with whoever cuts releases and never enters this repository.
//! An update whose signature does not verify is refused by the plugin
//! before a byte of it is run — so the worst a compromised host can do is
//! serve nothing.

use crate::diag::log_sync;
use tauri_plugin_updater::UpdaterExt;

/// What the Updates pane shows.
#[derive(serde::Serialize)]
pub struct UpdateStatus {
    /// The version this app is, so the pane can say what it is comparing.
    pub current: String,
    /// The version on offer, when one is newer than this app.
    pub available: Option<String>,
    /// The release notes that came with it, if any.
    pub notes: Option<String>,
    /// Set when the check could not be made — offline, no endpoint
    /// configured, a host that answered with rubbish. Reported rather than
    /// swallowed: "no update" and "could not ask" are different answers and
    /// a pane that shows the first for the second is lying quietly.
    pub error: Option<String>,
}

fn current_version(app: &tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// Asks the endpoint whether there is a newer signed build.
#[tauri::command]
pub async fn check_update(app: tauri::AppHandle) -> Result<UpdateStatus, String> {
    let current = current_version(&app);
    let updater = match app.updater() {
        Ok(u) => u,
        // No endpoint configured is the ordinary state of a dev build, not
        // a fault worth a red banner.
        Err(e) => {
            return Ok(UpdateStatus {
                current,
                available: None,
                notes: None,
                error: Some(format!("updates are not configured: {e}")),
            });
        }
    };
    match updater.check().await {
        Ok(Some(update)) => Ok(UpdateStatus {
            current,
            available: Some(update.version.clone()),
            notes: update.body.clone(),
            error: None,
        }),
        Ok(None) => Ok(UpdateStatus {
            current,
            available: None,
            notes: None,
            error: None,
        }),
        Err(e) => Ok(UpdateStatus {
            current,
            available: None,
            notes: None,
            error: Some(format!("{e}")),
        }),
    }
}

/// Downloads and installs the update the check found, then asks to restart.
///
/// The plugin verifies the signature against the compiled-in public key
/// before installing; a failure here means nothing was installed.
#[tauri::command]
pub async fn install_update(app: tauri::AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let Some(update) = updater.check().await.map_err(|e| e.to_string())? else {
        return Err("there is no update to install".into());
    };
    let version = update.version.clone();
    let mut downloaded = 0usize;
    update
        .download_and_install(
            |chunk, total| {
                downloaded += chunk;
                if let Some(total) = total
                    && total > 0
                    && downloaded % (512 * 1024) < chunk
                {
                    log_sync(&format!("update {version}: {downloaded}/{total} bytes"));
                }
            },
            || log_sync("update downloaded; installing"),
        )
        .await
        .map_err(|e| e.to_string())?;
    log_sync("update installed; restart to run it");
    Ok(())
}

/// Quits so the installed update is what starts next.
///
/// Separate from installing, and only ever called by a button the person
/// pressed: an app that restarts itself while a reply is half-written is
/// worse than one that waits to be asked.
#[tauri::command]
pub fn restart_for_update(app: tauri::AppHandle) {
    log_sync("restarting into the installed update");
    app.restart();
}
