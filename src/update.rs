use std::sync::mpsc;
use std::time::Duration;

use egui::Ui;
use serde::Deserialize;

const LATEST: &str =
    "https://api.github.com/repos/Project-Stateful-Zenith/iec104-workbench/releases/latest";

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    pub html_url: String,
}

/// The About window's update check. It runs only when the user presses the
/// button: the workbench is often on networks that must not see it call out.
#[derive(Default)]
pub enum Check {
    #[default]
    Idle,
    Running(mpsc::Receiver<Result<Release, String>>),
    Done(Result<Release, String>),
}

impl Check {
    pub fn ui(&mut self, ui: &mut Ui) {
        if let Self::Running(rx) = self
            && let Ok(result) = rx.try_recv()
        {
            *self = Self::Done(result);
        }
        ui.horizontal_wrapped(|ui| {
            let running = matches!(self, Self::Running(_));
            if ui
                .add_enabled(!running, egui::Button::new("Check for updates"))
                .clicked()
            {
                self.start(ui.ctx().clone());
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
            }
        });
    }

    fn start(&mut self, ctx: egui::Context) {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(latest());
            ctx.request_repaint();
        });
        *self = Self::Running(rx);
    }
}

/// The latest published release on GitHub. Blocks for up to 10 s.
fn latest() -> Result<Release, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let mut response = agent
        .get(LATEST)
        .header(
            "User-Agent",
            concat!("iec104-workbench/", env!("CARGO_PKG_VERSION")),
        )
        .call()
        .map_err(|e| e.to_string())?;
    response.body_mut().read_json().map_err(|e| e.to_string())
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
}
