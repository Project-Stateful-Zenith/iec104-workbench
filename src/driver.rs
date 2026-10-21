use std::sync::mpsc;

use chrono::{DateTime, Local};
use iec104::link::{Role, State};
use iec104::manager::{ConnectionManager, StationConfig};
use iec104_tls::tokio::TlsConnector;
use iec104_tokio::{Event, Handle, Options, TcpConnector, master};
use tokio::runtime::Runtime;

use crate::settings::Settings;

const CONN: usize = 0;

pub struct Notice {
    pub at: DateTime<Local>,
    pub kind: NoticeKind,
}

pub enum NoticeKind {
    Event(Event),
    Window(u16),
    Error(String),
    Log(String),
    /// The simulator reported a point's new value after a command.
    Echo {
        ioa: u32,
        value: iec104::asdu::Value,
    },
}

pub struct Driver {
    runtime: Runtime,
    handle: Handle,
    notices: mpsc::Sender<Notice>,
}

impl Driver {
    pub fn connect(
        settings: &Settings,
        notices: mpsc::Sender<Notice>,
        wake: impl Fn() + Send + 'static,
    ) -> Result<Self, String> {
        let params = settings.link()?;
        let options = Options {
            auto_start: settings.auto_start,
            trace: true,
            sizes: settings.sizes()?,
        };
        let manager = ConnectionManager::new(Role::Controlling, vec![StationConfig::new(params)])
            .map_err(|e| format!("{e:?}"))?;
        let tls = settings
            .tls
            .enabled
            .then(|| settings.tls_config())
            .transpose()?;
        let addresses = vec![settings.address()];
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (handle, mut events) = runtime
            .block_on(async {
                match tls {
                    Some(config) => {
                        master::spawn(manager, TlsConnector::new(config, addresses), options)
                    }
                    None => master::spawn(manager, TcpConnector::new(addresses), options),
                }
            })
            .map_err(|e| e.to_string())?;

        let forward = notices.clone();
        let window = handle.clone();
        runtime.spawn(async move {
            while let Some(event) = events.recv().await {
                let ask = matches!(
                    event,
                    Event::Sent { .. } | Event::Acknowledged { .. } | Event::State { .. }
                );
                let sent = forward.send(Notice::now(NoticeKind::Event(event)));
                if ask && let Ok(room) = window.window(CONN).await {
                    let _ = forward.send(Notice::now(NoticeKind::Window(room)));
                }
                if sent.is_err() {
                    break;
                }
                wake();
            }
        });
        Ok(Self {
            runtime,
            handle,
            notices,
        })
    }

    pub fn send(&self, asdu: Vec<u8>) {
        self.request(move |h| async move { h.send_asdu(CONN, asdu).await });
    }

    pub fn start(&self) {
        self.request(|h| async move { h.start(CONN).await });
    }

    pub fn stop(&self) {
        self.request(|h| async move { h.stop(CONN).await });
    }

    fn request<F>(&self, call: impl FnOnce(Handle) -> F)
    where
        F: Future<Output = Result<(), iec104_tokio::Error>> + Send + 'static,
    {
        let notices = self.notices.clone();
        let call = call(self.handle.clone());
        self.runtime.spawn(async move {
            if let Err(e) = call.await {
                let _ = notices.send(Notice::now(NoticeKind::Error(e.to_string())));
            }
        });
    }

    pub fn shutdown(self) {
        let handle = self.handle;
        self.runtime
            .block_on(async move { handle.shutdown().await });
    }
}

impl Notice {
    pub fn now(kind: NoticeKind) -> Self {
        Self {
            at: Local::now(),
            kind,
        }
    }
}

pub const fn state_name(state: State) -> &'static str {
    match state {
        State::Stopped => "stopped",
        State::PendingStarted => "starting",
        State::Started => "started",
        State::PendingStopped | State::PendingUnconfirmedStopped => "stopping",
        State::Terminated => "closed",
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use iec104::asdu::{Quality, Value};
    use iec104_tokio::server::{Application, PointConfig, Server, ServerConfig};

    use super::*;
    use crate::commands::{Context, Forms, SystemKind, Tab};
    use crate::points::Points;

    struct Accept;

    impl Application for Accept {
        fn operate(&self, _command: iec104::ct::Command) -> impl Future<Output = bool> + Send {
            std::future::ready(true)
        }
    }

    #[test]
    fn interrogates_a_station() {
        let station = tokio::runtime::Runtime::new().unwrap();
        let port = station.block_on(async {
            let config =
                ServerConfig::new(1).point(100, PointConfig::new(Value::Scaled(42)).untagged());
            let server = Server::spawn(config, Accept).unwrap();
            server
                .set(100, Value::Scaled(42), Quality::default())
                .await
                .unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(server.serve(listener));
            port
        });

        let settings = Settings {
            port,
            ..Settings::default()
        };
        let (tx, rx) = mpsc::channel();
        let driver = Driver::connect(&settings, tx, || {}).unwrap();
        let mut forms = Forms {
            tab: Tab::System,
            qualifier: "20".into(),
            ..Forms::default()
        };
        let ctx = Context {
            sizes: settings.sizes().unwrap(),
            common_address: 1,
            originator: 0,
        };

        let mut points = Points::default();
        let mut sent_gi = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while points.rows.is_empty() && Instant::now() < deadline {
            let Ok(notice) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            match notice.kind {
                NoticeKind::Event(Event::State {
                    state: State::Started,
                    ..
                }) if !sent_gi => {
                    driver.send(forms.build(&ctx).unwrap());
                    sent_gi = true;
                }
                NoticeKind::Event(Event::Asdu { asdu, .. }) => {
                    points.apply(&asdu, ctx.sizes, notice.at);
                }
                _ => {}
            }
        }
        assert_eq!(points.rows[&(1, 100)].value, "42");
        assert_eq!(points.rows[&(1, 100)].cause, "inrogen");

        forms.system = SystemKind::CounterInterrogation;
        forms.qualifier = "5".into();
        driver.send(forms.build(&ctx).unwrap());
        let mut confirmation = None;
        while confirmation.is_none() && Instant::now() < deadline {
            if let Ok(Notice {
                kind: NoticeKind::Event(Event::Asdu { asdu, .. }),
                ..
            }) = rx.recv_timeout(Duration::from_millis(100))
                && let Ok(header) = iec104::asdu::Header::parse(&asdu)
                && header.type_id == iec104::asdu::TypeId::CCiNa1
            {
                confirmation = Some(header);
            }
        }
        driver.shutdown();
        let header = confirmation.unwrap();
        assert_eq!(header.cause, iec104::asdu::Cause::ActivationCon);
        assert!(!header.negative);
    }
}
