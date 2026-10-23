use std::fmt::Write as _;
use std::sync::mpsc;
use std::time::Duration;

use egui::Ui;
use serde::Deserialize;

const LATEST: &str =
    "https://api.github.com/repos/Project-Stateful-Zenith/iec104-workbench/releases/latest";

/// The release binary built for this platform: `iec104-workbench-<tag>-<target>`.
const TARGET: Option<&str> = if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
    Some("x86_64-pc-windows-msvc")
} else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
    Some("x86_64-unknown-linux-gnu")
} else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
    Some("aarch64-apple-darwin")
} else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
    Some("x86_64-apple-darwin")
} else {
    None
};

#[derive(Debug, Clone, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

/// The About window's update check and install. Each runs only when the user
/// presses its button: the workbench is often on networks that must not see
/// it call out.
#[derive(Default)]
pub enum Check {
    #[default]
    Idle,
    Running(mpsc::Receiver<Result<Release, String>>),
    Done(Result<Release, String>),
    Installing(mpsc::Receiver<Result<String, String>>),
    Installed(Result<String, String>),
}

impl Check {
    pub fn ui(&mut self, ui: &mut Ui) {
        if let Self::Running(rx) = self
            && let Ok(result) = rx.try_recv()
        {
            *self = Self::Done(result);
        }
        if let Self::Installing(rx) = self
            && let Ok(result) = rx.try_recv()
        {
            *self = Self::Installed(result);
        }
        let mut install = None;
        ui.horizontal_wrapped(|ui| {
            let busy = matches!(self, Self::Running(_) | Self::Installing(_));
            if ui
                .add_enabled(!busy, egui::Button::new("Check for updates"))
                .clicked()
            {
                *self = Self::Running(spawn(ui.ctx().clone(), latest));
            }
            match self {
                Self::Idle => {}
                Self::Running(_) => {
                    ui.spinner();
                }
                Self::Done(Ok(release)) => {
                    match newer(&release.tag_name, env!("CARGO_PKG_VERSION")) {
                        Some(true) => {
                            ui.label(format!("{} is available:", release.tag_name));
                            ui.hyperlink_to("download page", &release.html_url);
                            if TARGET.is_some() && ui.button("Install").clicked() {
                                install = Some(release.clone());
                            }
                        }
                        Some(false) => {
                            ui.label("This is the latest release.");
                        }
                        None => {
                            ui.label(format!("The latest release is {}:", release.tag_name));
                            ui.hyperlink_to("release page", &release.html_url);
                        }
                    }
                }
                Self::Done(Err(e)) => {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("Could not check: {e}"));
                }
                Self::Installing(_) => {
                    ui.spinner();
                    ui.label("downloading…");
                }
                Self::Installed(Ok(tag)) => {
                    ui.label(format!("{tag} installed. Restart the workbench to use it."));
                }
                Self::Installed(Err(e)) => {
                    ui.colored_label(ui.visuals().warn_fg_color, format!("Not installed: {e}"));
                }
            }
        });
        if let Some(release) = install {
            *self = Self::Installing(spawn(ui.ctx().clone(), move || install_release(&release)));
        }
    }
}

/// Runs `work` on its own thread and repaints when it is done.
fn spawn<T: Send + 'static>(
    ctx: egui::Context,
    work: impl FnOnce() -> T + Send + 'static,
) -> mpsc::Receiver<T> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
        ctx.request_repaint();
    });
    rx
}

fn get(url: &str, timeout: Duration) -> Result<ureq::http::Response<ureq::Body>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .build()
        .into();
    agent
        .get(url)
        .header(
            "User-Agent",
            concat!("iec104-workbench/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| e.to_string())
}

/// The latest published release on GitHub. Blocks for up to 10 s.
fn latest() -> Result<Release, String> {
    get(LATEST, Duration::from_secs(10))?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    get(url, Duration::from_secs(300))?
        .body_mut()
        .with_config()
        .limit(limit)
        .read_to_vec()
        .map_err(|e| e.to_string())
}

/// Downloads this platform's binary of `release`, checks it against the
/// release's `SHA256SUMS` and puts it in place of the running one, which
/// keeps running until the user restarts. Returns the installed tag.
fn install_release(release: &Release) -> Result<String, String> {
    let target = TARGET.ok_or("no release binary is built for this platform")?;
    let name = format!(
        "iec104-workbench-{}-{target}{}",
        release.tag_name,
        std::env::consts::EXE_SUFFIX
    );
    let url = |file: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == file)
            .map(|a| a.browser_download_url.clone())
            .ok_or_else(|| format!("the release has no {file}"))
    };
    let sums = String::from_utf8(download(&url("SHA256SUMS")?, 64 * 1024)?)
        .map_err(|_| "SHA256SUMS is not text")?;
    let expected =
        sum_for(&sums, &name).ok_or_else(|| format!("SHA256SUMS has no line for {name}"))?;
    let binary = download(&url(&name)?, 256 * 1024 * 1024)?;
    if !sha256_hex(&binary).eq_ignore_ascii_case(expected) {
        return Err(format!(
            "{name} does not match its SHA-256 in SHA256SUMS; nothing was replaced"
        ));
    }
    let path = std::env::temp_dir().join(&name);
    std::fs::write(&path, &binary).map_err(|e| format!("{}: {e}", path.display()))?;
    let replaced =
        self_replace::self_replace(&path).map_err(|e| format!("replacing the running binary: {e}"));
    let _ = std::fs::remove_file(&path);
    replaced.map(|()| release.tag_name.clone())
}

/// The hash `sha256sum` wrote for `name`.
fn sum_for<'a>(sums: &'a str, name: &str) -> Option<&'a str> {
    sums.lines().find_map(|line| {
        let (hash, file) = line.split_once(' ')?;
        (file.trim_start_matches([' ', '*']) == name).then_some(hash)
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut hex = String::with_capacity(64);
    for b in digest.as_ref() {
        let _ = write!(hex, "{b:02x}");
    }
    hex
}

/// Whether the release tag (`v1.2.3`) is newer than `current` (`1.2.3`);
/// `None` when either is not three numbers.
fn newer(tag: &str, current: &str) -> Option<bool> {
    let parse = |version: &str| -> Option<(u64, u64, u64)> {
        let mut parts = version.trim_start_matches('v').split('.').map(str::parse);
        let triple = (
            parts.next()?.ok()?,
            parts.next()?.ok()?,
            parts.next()?.ok()?,
        );
        parts.next().is_none().then_some(triple)
    };
    Some(parse(tag)? > parse(current)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "needs GitHub"]
    fn fetches_the_latest_release() {
        let release = latest().unwrap();
        assert_eq!(newer(&release.tag_name, "0.0.1"), Some(true), "{release:?}");
        assert!(release.html_url.starts_with("https://github.com/"));
    }

    /// Downloads v1.0.0's Linux binary and checks it as the install does,
    /// without replacing anything.
    #[test]
    #[ignore = "needs GitHub"]
    fn a_release_binary_matches_its_sum() {
        let release: Release = get(
            "https://api.github.com/repos/Project-Stateful-Zenith/iec104-workbench/releases/tags/v1.0.0",
            Duration::from_secs(10),
        )
        .unwrap()
        .body_mut()
        .read_json()
        .unwrap();
        let url = |file: &str| {
            release
                .assets
                .iter()
                .find(|a| a.name == file)
                .unwrap()
                .browser_download_url
                .clone()
        };
        let name = "iec104-workbench-v1.0.0-x86_64-unknown-linux-gnu";
        let sums = String::from_utf8(download(&url("SHA256SUMS"), 64 * 1024).unwrap()).unwrap();
        let binary = download(&url(name), 256 * 1024 * 1024).unwrap();
        assert_eq!(sha256_hex(&binary), sum_for(&sums, name).unwrap());
    }

    #[test]
    fn compares_release_tags() {
        assert_eq!(newer("v1.0.1", "1.0.0"), Some(true));
        assert_eq!(newer("v1.10.0", "1.9.9"), Some(true));
        assert_eq!(newer("v2.0.0", "1.99.99"), Some(true));
        assert_eq!(newer("v1.0.0", "1.0.0"), Some(false));
        assert_eq!(newer("v0.6.0", "1.0.0"), Some(false));
        assert_eq!(newer("v1.1", "1.0.0"), None);
        assert_eq!(newer("v1.1.0-rc1", "1.0.0"), None);
        assert_eq!(newer("nightly", "1.0.0"), None);
    }

    #[test]
    fn finds_a_binary_sum() {
        let sums = "aa11  iec104-workbench-v1.0.0-x86_64-apple-darwin\n\
                    bb22 *iec104-workbench-v1.0.0-x86_64-pc-windows-msvc.exe\n\
                    cc33  iec104-workbench-v1.0.0-x86_64-unknown-linux-gnu\n";
        let sum = |name| sum_for(sums, name);
        assert_eq!(
            sum("iec104-workbench-v1.0.0-x86_64-pc-windows-msvc.exe"),
            Some("bb22")
        );
        assert_eq!(
            sum("iec104-workbench-v1.0.0-x86_64-unknown-linux-gnu"),
            Some("cc33")
        );
        assert_eq!(sum("iec104-workbench-v1.0.0-x86_64-unknown-linux"), None);
        assert_eq!(sum("iec104-workbench"), None);
    }

    #[test]
    fn hashes_like_sha256sum() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
