//! In-app update UI: startup check, update dialog, manual check.
//!
//! The update check runs once per fresh session (spawned lazily on the first
//! UI frame, on a background thread). There are no periodic background
//! checks while the app runs. Set `HELION_NO_UPDATE_CHECK=1` to skip the
//! startup check (CI, dev builds).

use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui::{self, RichText};

use super::toasts::{self, ToastKind};

/// Result of the background release check: the newer release and its install
/// plan, or None when already up to date.
pub type CheckResult = Result<Option<(helion_update::ReleaseInfo, helion_update::InstallPlan)>, String>;

/// What the updater is doing right now.
pub enum UpdateFlow {
    Idle,
    /// A release check is in flight. `manual` is true for Help-menu checks
    /// (those report "up to date" / errors via toast); startup checks stay
    /// silent unless an update is found.
    Checking { manual: bool },
    /// A newer release was found; the dialog is showing.
    Available {
        release: helion_update::ReleaseInfo,
        plan: helion_update::InstallPlan,
    },
    /// Download / verify / install is running on a background thread.
    Working { stage: String, version: String },
    /// Download-only path finished: the asset is verified and waiting.
    Downloaded { path: PathBuf, version: String },
    /// Something failed; the message is shown in the dialog.
    Failed(String),
}

impl Default for UpdateFlow {
    fn default() -> Self {
        UpdateFlow::Idle
    }
}

/// Events from the background install thread.
pub enum InstallEvent {
    Stage(&'static str),
    Done(InstallOutcome),
    Failed(String),
}

/// Install thread outcome.
pub enum InstallOutcome {
    /// New binary/bundle installed in place; restart with this executable.
    Restart { exe: PathBuf },
    /// Asset downloaded and verified for manual install.
    Downloaded { path: PathBuf },
}

/// Spawn the release check on a background thread.
pub fn spawn_check() -> mpsc::Receiver<CheckResult> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result: Result<Option<(helion_update::ReleaseInfo, helion_update::InstallPlan)>, helion_update::UpdateError> =
            (|| {
                let release = helion_update::check_for_update()?;
                match release {
                    None => Ok(None),
                    Some(r) => {
                        let plan = helion_update::plan_install(&r)?;
                        Ok(Some((r, plan)))
                    }
                }
            })();
        let _ = tx.send(result.map_err(|e| e.to_string()));
    });
    rx
}

/// Directory for download-only updates: ~/Downloads, else the temp dir.
fn user_download_dir() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join("Downloads");
        }
    }
    std::env::temp_dir()
}

/// Spawn the download + verify + install on a background thread.
pub fn spawn_install(
    release: helion_update::ReleaseInfo,
    plan: helion_update::InstallPlan,
) -> mpsc::Receiver<InstallEvent> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let result = run_install(&release, &plan, &tx);
        match result {
            Ok(outcome) => {
                let _ = tx.send(InstallEvent::Done(outcome));
            }
            Err(e) => {
                let _ = tx.send(InstallEvent::Failed(e.to_string()));
            }
        }
    });
    rx
}

fn run_install(
    release: &helion_update::ReleaseInfo,
    plan: &helion_update::InstallPlan,
    tx: &mpsc::Sender<InstallEvent>,
) -> Result<InstallOutcome, helion_update::UpdateError> {
    let asset_name = plan.asset.name.clone();

    // Keep the staging dir alive for the whole install.
    let _stage_guard: Option<tempfile::TempDir>;
    let dest: PathBuf = match &plan.mode {
        helion_update::InstallMode::DownloadOnly { .. } => {
            let dir = user_download_dir();
            std::fs::create_dir_all(&dir)?;
            dir.join(&asset_name)
        }
        helion_update::InstallMode::InPlace(_) => {
            let dir = tempfile::TempDir::new()?;
            let path = dir.path().join(&asset_name);
            _stage_guard = Some(dir);
            path
        }
    };

    let _ = tx.send(InstallEvent::Stage("Downloading release…"));
    helion_update::download_to_file(&plan.asset.download_url, &dest)?;

    let _ = tx.send(InstallEvent::Stage("Verifying checksum…"));
    let sums = helion_update::fetch_sha256sums(release)?;
    let map = helion_update::parse_sha256sums(&sums);
    let expected = helion_update::lookup_checksum(&map, &asset_name)?;
    let bytes = std::fs::read(&dest)?;
    helion_update::verify_bytes(&bytes, expected, &asset_name)?;

    let _ = tx.send(InstallEvent::Stage("Installing…"));
    match &plan.mode {
        helion_update::InstallMode::InPlace(kind) => {
            let exe = match kind {
                helion_update::InstallKind::Binary { .. } => {
                    let target = helion_update::target_triple();
                    helion_update::install_binary_from_tarball(
                        &dest,
                        &release.version,
                        &target,
                    )?
                }
                helion_update::InstallKind::MacBundle { bundle_path } => {
                    helion_update::install_bundle_from_zip(&dest, bundle_path)?;
                    bundle_path.join("Contents").join("MacOS").join("helion-ide")
                }
            };
            Ok(InstallOutcome::Restart { exe })
        }
        helion_update::InstallMode::DownloadOnly { .. } => {
            Ok(InstallOutcome::Downloaded { path: dest })
        }
    }
}

/// Spawn the new executable with the same CLI args, then close this window.
/// Called after a successful in-place install. The editor is saved by the
/// caller before this runs.
pub fn restart_with(exe: &PathBuf, ctx: &egui::Context, toasts: &mut toasts::ToastQueue) {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match std::process::Command::new(exe).args(&args).spawn() {
        Ok(_) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
        Err(e) => toasts.push(
            ToastKind::Error,
            format!("Update installed but restart failed: {e}"),
        ),
    }
}

/// Paint the update dialog for the current flow state.
/// Facts only in the copy: versions, paths, and what happens next.
pub fn paint_update_dialog(ctx: &egui::Context, app: &mut super::HelionIde) {
    let theme = app.theme;
    match &app.update_flow {
        UpdateFlow::Available { release, plan } => {
            let version = release.version.to_string();
            let current = env!("CARGO_PKG_VERSION");
            let download_only = matches!(
                plan.mode,
                helion_update::InstallMode::DownloadOnly { .. }
            );
            let reason = match &plan.mode {
                helion_update::InstallMode::DownloadOnly { reason } => Some(reason.clone()),
                _ => None,
            };
            let mut close = false;
            let mut start_install = false;
            egui::Window::new("Update available")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_max_width(380.0);
                    ui.label(RichText::new(format!(
                        "Helion v{version} is available (you are running v{current})."
                    )));
                    if download_only {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(format!(
                                "The installed copy cannot be replaced automatically: {}. \
                                 The release file will be downloaded and verified for manual install.",
                                reason.unwrap_or_default()
                            ))
                            .small()
                            .color(theme.dim),
                        );
                    }
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button("Update").clicked() {
                            start_install = true;
                        }
                        if ui.button("Later").clicked() {
                            close = true;
                        }
                    });
                });
            if start_install {
                if let UpdateFlow::Available { release, plan } =
                    std::mem::replace(&mut app.update_flow, UpdateFlow::Idle)
                {
                    let version = release.version.to_string();
                    app.update_install_rx = Some(spawn_install(release, plan));
                    app.update_flow = UpdateFlow::Working {
                        stage: "Starting…".to_string(),
                        version,
                    };
                }
            } else if close {
                app.update_flow = UpdateFlow::Idle;
            }
        }
        UpdateFlow::Working { stage, version } => {
            let stage = stage.clone();
            let version = version.clone();
            egui::Window::new("Updating Helion")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_max_width(380.0);
                    ui.label(format!("Updating to v{version}. {stage}"));
                    ui.add_space(6.0);
                    ui.spinner();
                });
        }
        UpdateFlow::Downloaded { path, version } => {
            let path = path.clone();
            let version = version.clone();
            let mut close = false;
            egui::Window::new("Update downloaded")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(format!(
                        "Helion v{version} downloaded and verified:\n{}",
                        path.display()
                    ));
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new("Replace the installed copy manually to update.")
                            .small()
                            .color(theme.dim),
                    );
                    ui.add_space(10.0);
                    if ui.button("OK").clicked() {
                        close = true;
                    }
                });
            if close {
                app.update_flow = UpdateFlow::Idle;
            }
        }
        UpdateFlow::Failed(msg) => {
            let msg = msg.clone();
            let mut close = false;
            egui::Window::new("Update failed")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
                .show(ctx, |ui| {
                    ui.set_max_width(420.0);
                    ui.label(format!("The update did not complete: {msg}"));
                    ui.add_space(10.0);
                    if ui.button("OK").clicked() {
                        close = true;
                    }
                });
            if close {
                app.update_flow = UpdateFlow::Idle;
            }
        }
        UpdateFlow::Idle | UpdateFlow::Checking { .. } => {}
    }
}
