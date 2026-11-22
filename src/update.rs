//! Update check: compares the latest GitHub release with this version and installs its MSI.
//! Uses curl.exe, which ships with Windows 10 (1803+), so no HTTP library is needed.

use eframe::egui;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

const REPO: &str = "https://github.com/thomas3577/notepad";

#[derive(Clone, Default)]
pub enum Status {
    #[default]
    UpToDate,
    Available(String),
    Downloading,
    Failed(String),
}

#[derive(Clone)]
pub struct Updater {
    status: Arc<Mutex<Status>>,
    ctx: egui::Context,
}

impl Updater {
    /// Checks for a newer release in the background; the status bar shows the result.
    pub fn start(ctx: egui::Context) -> Self {
        let u = Updater { status: Default::default(), ctx };
        let bg = u.clone();
        std::thread::spawn(move || {
            if let Some(v) = latest().filter(|v| newer(v, env!("CARGO_PKG_VERSION"))) {
                bg.set(Status::Available(v));
            }
        });
        u
    }

    pub fn status(&self) -> Status {
        self.status.lock().unwrap().clone()
    }

    fn set(&self, s: Status) {
        *self.status.lock().unwrap() = s;
        self.ctx.request_repaint();
    }

    /// Downloads the MSI, starts it and closes Notepad (the session keeps all tabs);
    /// Notepad starts again when the installer is done.
    pub fn install(&self, version: String) {
        self.set(Status::Downloading);
        let bg = self.clone();
        std::thread::spawn(move || match run_installer(&version) {
            Ok(()) => {
                bg.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                bg.ctx.request_repaint();
            }
            Err(e) => bg.set(Status::Failed(e)),
        });
    }
}

/// Programs from System32 by full path, run from System32: never from the exe's or the
/// working directory (which may be Downloads).
fn system(name: &str) -> Command {
    let dir = PathBuf::from(std::env::var_os("SystemRoot").unwrap_or("C:\\Windows".into())).join("System32");
    let mut c = Command::new(dir.join(name));
    c.current_dir(dir);
    #[cfg(windows)]
    std::os::windows::process::CommandExt::creation_flags(&mut c, 0x0800_0000); // CREATE_NO_WINDOW
    c
}

/// Version of the latest release: /releases/latest redirects to /releases/tag/v1.2.3.
fn latest() -> Option<String> {
    let out = system("curl.exe")
        .args(["-sfLI", "-w", "%{url_effective}", &format!("{REPO}/releases/latest")])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let stdout = String::from_utf8(out.stdout).ok()?;
    tag_version(stdout.rsplit('\n').next()?)
}

fn tag_version(url: &str) -> Option<String> {
    Some(url.rsplit_once("/tag/")?.1.trim().trim_start_matches('v').to_string())
}

fn newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| v.split('.').map(|n| n.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>();
    parse(latest) > parse(current)
}

/// Asset name as built by .github/workflows/release.yml.
fn run_installer(version: &str) -> Result<(), String> {
    let dir = std::env::temp_dir();
    let msi = dir.join(format!("notepad-{version}-x64.msi"));
    let url = format!("{REPO}/releases/download/v{version}/notepad-{version}-x64.msi");
    let downloaded = system("curl.exe").args(["-sfL", "-o"]).arg(&msi).arg(&url).status().is_ok_and(|s| s.success());
    if !downloaded {
        return Err(format!("Download failed: {url}"));
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // a batch file waits for msiexec (a GUI program), then starts the new version;
    // % would be expanded in a batch file
    let q = |p: &std::path::Path| p.display().to_string().replace('%', "%%");
    let script = dir.join("notepad-update.cmd");
    std::fs::write(&script, format!("msiexec /i \"{}\" /passive\r\nstart \"\" \"{}\"\r\n", q(&msi), q(&exe)))
        .map_err(|e| e.to_string())?;
    system("cmd.exe").arg("/c").arg(&script).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("0.7.10", "0.7.2"));
        assert!(newer("1.0.0", "0.9.9"));
        assert!(!newer("0.7.2", "0.7.2"));
        assert!(!newer("0.7.1", "0.7.2"));
        assert!(!newer("garbage", "0.7.2"));
        assert_eq!(tag_version("https://github.com/thomas3577/notepad/releases/tag/v0.7.3").as_deref(), Some("0.7.3"));
        assert_eq!(tag_version("https://github.com/thomas3577/notepad/releases"), None);
    }
}
