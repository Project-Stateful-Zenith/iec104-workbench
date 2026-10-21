use std::sync::mpsc;

use chrono::{DateTime, Local};
use iec104::link::{Role, State};
use iec104::manager::{ConnectionManager, StationConfig};
use iec104::params::LinkParams;
use iec104::redundancy::{RedundancyGroup, Replay, ReplayOutcome, Switchover};
use iec104_tls::tokio::TlsConnector;
use iec104_tokio::{Connector, Event, Events, Handle, Options, TcpConnector, master};
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
    Status(String),
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
        let tls = settings
            .tls
            .enabled
            .then(|| settings.tls_config())
            .transpose()?;
        let addresses = settings.addresses();
        let members = addresses.len();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let (handle, mut events) = runtime.block_on(async {
            match tls {
                Some(config) => spawn(
                    params,
                    members,
                    TlsConnector::new(config, addresses),
                    options,
                ),
                None => spawn(params, members, TcpConnector::new(addresses), options),
            }
        })?;

        let forward = notices.clone();
        let window = handle.clone();
        runtime.spawn(async move {
            // Members whose started connection failed with unconfirmed data,
            // replayed once the group names a started connection again (10.6).
            let mut failed = Vec::new();
            while let Some(event) = events.recv().await {
                let ask = match event {
                    Event::Sent { conn, .. }
                    | Event::Acknowledged { conn }
                    | Event::State { conn, .. } => Some(conn),
                    _ => None,
                };
                match event {
                    Event::StartedConnectionFailed { conn, unconfirmed }
                    | Event::Switchover(
                        Switchover::Automatic {
                            failed: conn,
                            unconfirmed,
                            ..
                        }
                        | Switchover::NoStoppedConnection {
                            failed: conn,
                            unconfirmed,
                        }
                        | Switchover::LeftToTheControllingStation {
                            failed: conn,
                            unconfirmed,
                        },
                    ) if unconfirmed > 0 && !failed.contains(&conn) => failed.push(conn),
                    _ => {}
                }
                let started = matches!(
                    event,
                    Event::State {
                        state: State::Started,
                        ..
                    }
                );
                let sent = forward.send(Notice::now(NoticeKind::Event(event)));
                if let Some(conn) = ask
                    && let Ok(room) = window.window(conn).await
                {
                    let _ = forward.send(Notice::now(NoticeKind::Window(room)));
                }
                if started {
                    for conn in std::mem::take(&mut failed) {
                        let text = match window
                            .retransmit_unconfirmed(conn, Replay::Retransmit)
                            .await
                        {
                            Ok(ReplayOutcome::Retransmitted { .. }) => {
                                format!("conn {conn}'s unconfirmed data sent again")
                            }
                            Ok(_) => continue,
                            Err(e) => format!("retransmitting conn {conn}'s unconfirmed data: {e}"),
                        };
                        let _ = forward.send(Notice::now(NoticeKind::Status(text)));
                    }
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

    /// A redundancy group puts user data on its started connection, whichever
    /// member is named.
    pub fn send(&self, asdu: Vec<u8>) {
        self.request(move |h| async move { h.send_asdu(CONN, asdu).await });
    }

    /// On a redundancy group, starting a member closes the others that are
    /// not stopped (10.7): the manual switchover.
    pub fn start(&self, conn: usize) {
        self.request(move |h| async move { h.start(conn).await });
    }

    pub fn stop(&self, conn: usize) {
        self.request(move |h| async move { h.stop(conn).await });
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

/// One address is a single connection; more are a redundancy group's members
/// (clause 10), all to the same station.
fn spawn<K: Connector>(
    params: LinkParams,
    members: usize,
    connector: K,
    options: Options,
) -> Result<(Handle, Events), String> {
    let spawned = if members > 1 {
        let group = RedundancyGroup::uniform(Role::Controlling, params, members)
            .map_err(|e| format!("{e:?}"))?;
        master::spawn_redundant(group, connector, options)
    } else {
        let manager = ConnectionManager::new(Role::Controlling, vec![StationConfig::new(params)])
            .map_err(|e| format!("{e:?}"))?;
        master::spawn(manager, connector, options)
    };
    spawned.map_err(|e| e.to_string())
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
pub mod tests {
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

    /// A station on a free local port with CA 1 and IOA 100 = scaled 42.
    pub fn station() -> (tokio::runtime::Runtime, u16) {
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
        (station, port)
    }

    #[test]
    fn interrogates_a_station() {
        let (_station, port) = station();
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

    /// Waits for the first event `found` returns `Some` for.
    fn wait<T>(rx: &mpsc::Receiver<Notice>, mut found: impl FnMut(Event) -> Option<T>) -> T {
        for _ in 0..100 {
            if let Ok(Notice {
                kind: NoticeKind::Event(event),
                ..
            }) = rx.recv_timeout(Duration::from_millis(100))
                && let Some(value) = found(event)
            {
                return value;
            }
        }
        panic!("timed out");
    }

    #[test]
    fn switches_over_in_a_redundancy_group() {
        let station = tokio::runtime::Runtime::new().unwrap();
        let port = station.block_on(async {
            let mut config =
                ServerConfig::new(1).point(100, PointConfig::new(Value::Scaled(42)).untagged());
            config.members = 2;
            let server = Server::spawn(config, Accept).unwrap();
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = listener.local_addr().unwrap().port();
            tokio::spawn(server.serve(listener));
            port
        });

        let settings = Settings {
            port,
            t0: 1,
            redundant: vec![format!("127.0.0.1:{port}")],
            ..Settings::default()
        };
        let (tx, rx) = mpsc::channel();
        let driver = Driver::connect(&settings, tx, || {}).unwrap();
        let interrogation = Forms {
            tab: Tab::System,
            qualifier: "20".into(),
            ..Forms::default()
        }
        .build(&Context {
            sizes: settings.sizes().unwrap(),
            common_address: 1,
            originator: 0,
        })
        .unwrap();
        let interrogated_on = |rx: &mpsc::Receiver<Notice>| {
            driver.send(interrogation.clone());
            wait(rx, |event| match event {
                Event::Asdu { conn, asdu }
                    if iec104::asdu::Header::parse(&asdu)
                        .is_ok_and(|h| h.type_id == iec104::asdu::TypeId::MMeNb1) =>
                {
                    Some(conn)
                }
                _ => None,
            })
        };
        let started = |rx: &mpsc::Receiver<Notice>| {
            wait(rx, |event| match event {
                Event::State {
                    conn,
                    state: State::Started,
                } => Some(conn),
                _ => None,
            })
        };
        // Either member may come up first and be the one started.
        let first = started(&rx);
        let other = 1 - first;
        assert_eq!(interrogated_on(&rx), first);

        // Losing the started connection starts the other one.
        driver.runtime.block_on(driver.handle.close(first)).unwrap();
        assert_eq!(started(&rx), other);
        assert_eq!(interrogated_on(&rx), other);

        // STARTDT on the redialed member is the manual switchover back.
        for _ in 0..100 {
            if driver.runtime.block_on(driver.handle.state(first)).unwrap() == State::Stopped {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        driver.start(first);
        wait(&rx, |event| {
            matches!(event, Event::Disconnected { conn, .. } if conn == other).then_some(())
        });
        assert_eq!(started(&rx), first);
        assert_eq!(interrogated_on(&rx), first);
        driver.shutdown();
    }
}
