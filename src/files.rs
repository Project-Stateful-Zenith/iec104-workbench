use std::path::PathBuf;

use iec104::asdu::{Known, Nof, TimeTag};
use iec104::ft::receive::{DirectoryCall, DirectoryFile, Failure, Progress, Receiver};
use iec104::ft::{FileTransferProcedure, FtError, Selection};

use crate::commands::Context;

enum Session {
    Directory(DirectoryCall),
    File {
        receiver: Receiver,
        ioa: u32,
        nof: u16,
        path: PathBuf,
    },
}

pub struct Files {
    pub directory_ioa: String,
    pub file_ioa: String,
    pub file_nof: String,
    pub listing: Vec<DirectoryFile>,
    pub status: String,
    session: Option<Session>,
}

impl Default for Files {
    fn default() -> Self {
        Self {
            directory_ioa: "0".into(),
            file_ioa: "0".into(),
            file_nof: "1".into(),
            listing: Vec::new(),
            status: String::new(),
            session: None,
        }
    }
}

const fn procedure(ctx: &Context) -> FileTransferProcedure {
    FileTransferProcedure::controlling(Selection::MONITOR, ctx.common_address, ctx.originator)
        .with_address_sizes(ctx.sizes)
}

fn failure_text(failure: &Failure) -> String {
    match failure {
        Failure::FileNotReady { .. } => "the station says the file is not ready".into(),
        Failure::SectionNotReady { nos, .. } => {
            format!("the station says section {} is not ready", nos.value)
        }
        Failure::Refused { cause } => format!("refused by the station (cause {})", cause.to_u8()),
        Failure::FileChecksum { expected, got } => format!(
            "file checksum {:02X} does not match the station's {:02X}",
            expected.value, got.value
        ),
        Failure::UnknownLsq(lsq) => format!("unknown LSQ {}", lsq.value),
    }
}

impl Files {
    pub const fn busy(&self) -> bool {
        self.session.is_some()
    }

    pub fn call_directory(&mut self, ctx: &Context) -> Result<Vec<u8>, String> {
        let ioa = self
            .directory_ioa
            .trim()
            .parse()
            .map_err(|_| "directory IOA: not a number")?;
        let mut call = DirectoryCall::new(procedure(ctx));
        let asdu = call.start(ioa).map_err(|e| format!("{e:?}"))?;
        self.listing.clear();
        self.session = Some(Session::Directory(call));
        self.status = format!("directory at IOA {ioa} called");
        Ok(asdu)
    }

    pub fn download(
        &mut self,
        ctx: &Context,
        ioa: u32,
        nof: u16,
        path: PathBuf,
    ) -> Result<Vec<u8>, String> {
        let mut receiver = Receiver::new(procedure(ctx), ioa, Nof { value: nof });
        let asdu = receiver.start().map_err(|e| format!("{e:?}"))?;
        self.status = format!("file {nof} at IOA {ioa} selected");
        self.session = Some(Session::File {
            receiver,
            ioa,
            nof,
            path,
        });
        Ok(asdu)
    }

    pub fn cancel(&mut self) {
        if self.session.take().is_some() {
            self.status = "cancelled".into();
        }
    }

    pub fn progress(&self) -> Option<String> {
        match self.session.as_ref()? {
            Session::Directory(_) => Some("waiting for the directory".into()),
            Session::File {
                receiver, ioa, nof, ..
            } => Some(receiver.length().map_or_else(
                || format!("file {nof} at IOA {ioa}: waiting for file ready"),
                |length| {
                    format!(
                        "file {nof} at IOA {ioa}: {} / {length} octets",
                        receiver.received()
                    )
                },
            )),
        }
    }

    pub fn on_asdu(&mut self, asdu: &[u8]) -> Vec<Vec<u8>> {
        let known = Known::EVERYTHING;
        match &mut self.session {
            None => Vec::new(),
            Some(Session::Directory(call)) => match call.on_asdu(asdu, &known) {
                Ok(step) => {
                    match step.progress {
                        Progress::Running => {}
                        Progress::Complete(files) => {
                            self.status = format!("directory: {} entries", files.len());
                            self.listing = files;
                            self.session = None;
                        }
                        Progress::Failed(failure) => {
                            self.status = format!("directory: {}", failure_text(&failure));
                            self.session = None;
                        }
                    }
                    step.send
                }
                Err(e) => self.ignore(&e),
            },
            Some(Session::File { receiver, path, .. }) => match receiver.on_asdu(asdu, &known) {
                Ok(step) => {
                    match step.progress {
                        Progress::Running => {}
                        Progress::Complete(file) => {
                            self.status = match std::fs::write(&*path, &file) {
                                Ok(()) => {
                                    format!("saved {} octets to {}", file.len(), path.display())
                                }
                                Err(e) => format!("{}: {e}", path.display()),
                            };
                            self.session = None;
                        }
                        Progress::Failed(failure) => {
                            self.status = format!("download failed: {}", failure_text(&failure));
                            self.session = None;
                        }
                    }
                    step.send
                }
                Err(e) => self.ignore(&e),
            },
        }
    }

    /// A file-transfer frame that does not belong to the session is reported
    /// and skipped; the session goes on.
    fn ignore(&mut self, error: &FtError) -> Vec<Vec<u8>> {
        if !matches!(error, FtError::Layout(_)) {
            self.status = format!("ignored a frame: {error:?}");
        }
        Vec::new()
    }
}

pub fn entry_time(file: &DirectoryFile) -> String {
    crate::points::time_text(TimeTag::Cp56(file.entry.time))
}

#[cfg(test)]
mod tests {
    use iec104::asdu::{AddressSizes, Cause, Frq, Lof, Los, Lsq, Nos, Segment, Sof, Srq};
    use iec104::ft::{DirectoryEntry, Frame, checksum};

    use super::*;

    const CTX: Context = Context {
        sizes: AddressSizes::CONVENTIONAL,
        common_address: 1,
        originator: 0,
    };

    fn station(frame: &Frame) -> Vec<u8> {
        FileTransferProcedure::controlled(Selection::MONITOR, 1, 0)
            .build(frame)
            .unwrap()
    }

    /// Against lib60870's file server (`IEC104_LIB60870_SERVER`, built by
    /// zenith-iec104's `scripts/interop-setup.sh`): 800 octets in two sections.
    #[test]
    #[ignore = "needs the lib60870 peer"]
    fn downloads_from_lib60870() {
        use std::sync::mpsc;
        use std::time::{Duration, Instant};

        use iec104_tokio::Event;

        use crate::driver::{Driver, NoticeKind};
        use crate::settings::Settings;

        let server = std::env::var("IEC104_LIB60870_SERVER").unwrap();
        let mut peer = std::process::Command::new(server)
            .args(["24120", "30"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let settings = Settings {
            port: 24120,
            ..Settings::default()
        };
        let (tx, rx) = mpsc::channel();
        let driver = Driver::connect(&settings, tx, || {}).unwrap();
        let path = std::env::temp_dir().join("iec104-workbench-lib60870.bin");
        let mut files = Files::default();
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut started = false;
        while Instant::now() < deadline {
            let Ok(notice) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            match notice.kind {
                NoticeKind::Event(Event::State {
                    state: iec104::link::State::Started,
                    ..
                }) if !started => {
                    driver.send(files.download(&CTX, 30000, 1, path.clone()).unwrap());
                    started = true;
                }
                NoticeKind::Event(Event::Asdu { asdu, .. }) => {
                    for reply in files.on_asdu(&asdu) {
                        driver.send(reply);
                    }
                    if started && !files.busy() {
                        break;
                    }
                }
                _ => {}
            }
        }
        driver.shutdown();
        let _ = peer.kill();
        let _ = peer.wait();
        let saved = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        let expected: Vec<u8> = (0..2u8)
            .flat_map(|n| {
                (0..400u16).map(move |i| u8::try_from((i * u16::from(n + 1)) % 256).unwrap())
            })
            .collect();
        assert_eq!(saved, expected, "{}", files.status);
    }

    #[test]
    fn lists_and_downloads() {
        let mut files = Files::default();
        files.call_directory(&CTX).unwrap();
        let listing = station(&Frame::Directory {
            ioa: 0,
            cause: Cause::Request,
            files: vec![DirectoryEntry {
                nof: Nof { value: 1 },
                lof: Lof { value: 3 },
                sof: Sof {
                    last_file_of_directory: true,
                    ..Sof::default()
                },
                time: iec104::asdu::Cp56Time2a::default(),
            }],
        });
        assert!(
            files.on_asdu(&listing).is_empty(),
            "a directory is not answered"
        );
        assert_eq!(files.listing.len(), 1);
        assert!(!files.busy());

        let path = std::env::temp_dir().join("iec104-workbench-files-test.bin");
        files.download(&CTX, 0, 1, path.clone()).unwrap();
        let (ioa, nof, nos) = (0, Nof { value: 1 }, Nos { value: 1 });
        let content = [1u8, 2, 3];
        for frame in [
            Frame::FileReady {
                ioa,
                nof,
                lof: Lof { value: 3 },
                frq: Frq::default(),
            },
            Frame::SectionReady {
                ioa,
                nof,
                nos,
                lof: Lof { value: 3 },
                srq: Srq::default(),
            },
            Frame::Segment {
                ioa,
                segment: Segment {
                    nof,
                    nos,
                    los: Los { value: 3 },
                    payload: content.to_vec(),
                },
            },
            Frame::LastSection {
                ioa,
                nof,
                nos,
                lsq: Lsq {
                    value: Lsq::SECTION_WITHOUT_DEACTIVATION,
                },
                chs: checksum(&content),
            },
            Frame::LastSection {
                ioa,
                nof,
                nos,
                lsq: Lsq {
                    value: Lsq::FILE_WITHOUT_DEACTIVATION,
                },
                chs: checksum(&content),
            },
        ] {
            files.on_asdu(&station(&frame));
        }
        let saved = std::fs::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(saved, content);
        assert!(!files.busy());
        assert!(
            files.status.starts_with("saved 3 octets"),
            "{}",
            files.status
        );
    }
}
