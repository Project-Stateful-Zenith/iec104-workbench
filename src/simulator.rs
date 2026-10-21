use std::io;
use std::pin::Pin;
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};

use iec104::asdu::{Cause, DoubleCommandState, DoublePointState, Nva, Quality, Value};
use iec104::ct::{Command, CommandMode, CommandValue, Family};
use iec104_tls::TlsConfig;
use iec104_tokio::Event;
use iec104_tokio::server::{Application, PointConfig, Server, ServerConfig};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;

use crate::driver::{Notice, NoticeKind};
use crate::settings::Settings;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Single,
    Double,
    Step,
    Bitstring,
    Normalized,
    Scaled,
    Float,
    Counter,
}

impl Kind {
    pub const ALL: [(Self, &'static str); 8] = [
        (Self::Single, "Single point"),
        (Self::Double, "Double point"),
        (Self::Step, "Step position"),
        (Self::Bitstring, "Bitstring"),
        (Self::Normalized, "Normalized"),
        (Self::Scaled, "Scaled"),
        (Self::Float, "Float"),
        (Self::Counter, "Integrated total"),
    ];

    pub const fn family(self) -> Option<Family> {
        match self {
            Self::Single => Some(Family::Single),
            Self::Double => Some(Family::Double),
            Self::Step => Some(Family::RegulatingStep),
            Self::Bitstring => Some(Family::Bitstring),
            Self::Normalized => Some(Family::SetPointNormalized),
            Self::Scaled => Some(Family::SetPointScaled),
            Self::Float => Some(Family::SetPointFloat),
            Self::Counter => None,
        }
    }
}

/// A point of the simulated station. `value` is text so the table can edit it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SimPoint {
    pub ioa: u32,
    pub kind: Kind,
    pub value: String,
    pub invalid: bool,
    /// Interrogation group 1–16, 0 for station interrogation only.
    pub group: u8,
    pub tagged: bool,
    pub commands: bool,
}

impl SimPoint {
    pub fn new(ioa: u32) -> Self {
        Self {
            ioa,
            kind: Kind::Single,
            value: "0".into(),
            invalid: false,
            group: 0,
            tagged: true,
            commands: false,
        }
    }

    pub fn parse(&self) -> Result<Value, String> {
        let text = self.value.trim();
        let bad = || format!("IOA {}: {text:?} is not a {:?} value", self.ioa, self.kind);
        Ok(match self.kind {
            Kind::Single => Value::Single(match text {
                "0" => false,
                "1" => true,
                _ => return Err(bad()),
            }),
            Kind::Double => Value::Double(DoublePointState::from_bits(
                text.parse::<u8>()
                    .ok()
                    .filter(|v| *v <= 3)
                    .ok_or_else(bad)?,
            )),
            Kind::Step => Value::StepPosition {
                value: text
                    .parse()
                    .ok()
                    .filter(|v| (-64..=63).contains(v))
                    .ok_or_else(bad)?,
                transient: false,
            },
            Kind::Bitstring => Value::Bitstring(
                u32::from_str_radix(text.trim_start_matches("0x"), 16).map_err(|_| bad())?,
            ),
            Kind::Normalized => Value::Normalized(
                text.parse::<f64>()
                    .ok()
                    .filter(|v| (-1.0..1.0).contains(v))
                    .ok_or_else(bad)?,
            ),
            Kind::Scaled => Value::Scaled(text.parse().map_err(|_| bad())?),
            Kind::Float => Value::Float(text.parse().map_err(|_| bad())?),
            Kind::Counter => Value::Counter {
                reading: text.parse().map_err(|_| bad())?,
                sequence: 0,
                carry: false,
                adjusted: false,
            },
        })
    }

    pub fn quality(&self) -> Quality {
        Quality {
            invalid: self.invalid,
            ..Quality::default()
        }
    }
}

/// The text [`SimPoint::parse`] reads back as `value`.
pub fn value_text(value: &Value) -> String {
    match *value {
        Value::Single(on) => u8::from(on).to_string(),
        Value::Double(state) => state.to_bits().to_string(),
        Value::StepPosition { value, .. } => value.to_string(),
        Value::Bitstring(bits) => format!("{bits:08X}"),
        Value::Normalized(v) => v.to_string(),
        Value::Scaled(v) => v.to_string(),
        Value::Float(v) => v.to_string(),
        Value::Counter { reading, .. } => reading.to_string(),
        other => format!("{other:?}"),
    }
}

struct Station {
    server: Arc<OnceLock<Server>>,
    notices: mpsc::Sender<Notice>,
    wake: Wake,
    echo: bool,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

impl Station {
    fn log(&self, line: String) {
        let _ = self.notices.send(Notice::now(NoticeKind::Log(line)));
        (self.wake)();
    }
}

impl Application for Station {
    fn accept(&self, command: &Command) -> bool {
        self.log(format!("command IOA {}: {:?}", command.ioa, command.value));
        true
    }

    async fn operate(&self, command: Command) -> bool {
        if !self.echo {
            return true;
        }
        let Some(server) = self.server.get() else {
            return false;
        };
        let Ok(point) = server.point(command.ioa).await else {
            return true;
        };
        let Some(value) = returned(command.value, point.value) else {
            return true;
        };
        let quality = point.quality.unwrap_or_default();
        let reported = server
            .report_with(command.ioa, value, quality, None, Cause::ReturnInfoRemote)
            .await;
        if let Err(e) = reported {
            self.log(format!(
                "IOA {}: return information not sent: {e}",
                command.ioa
            ));
            return true;
        }
        let _ = self.notices.send(Notice::now(NoticeKind::Echo {
            ioa: command.ioa,
            value,
        }));
        (self.wake)();
        true
    }
}

fn returned(command: CommandValue, current: Value) -> Option<Value> {
    Some(match (command, current) {
        (CommandValue::Single { sco }, Value::Single(_)) => Value::Single(sco.scs),
        (CommandValue::Double { dco }, Value::Double(_)) => Value::Double(match dco.dcs {
            DoubleCommandState::On => DoublePointState::On,
            DoubleCommandState::Off => DoublePointState::Off,
            _ => return None,
        }),
        (CommandValue::RegulatingStep { rco }, Value::StepPosition { value, transient }) => {
            let step = match rco.rcs {
                iec104::asdu::RegulatingStepState::Higher => 1,
                iec104::asdu::RegulatingStepState::Lower => -1,
                _ => return None,
            };
            Value::StepPosition {
                value: value.saturating_add(step).clamp(-64, 63),
                transient,
            }
        }
        (CommandValue::SetPointNormalized { nva, .. }, Value::Normalized(_)) => {
            Value::Normalized(Nva::to_scaled(&nva))
        }
        (CommandValue::SetPointScaled { sva, .. }, Value::Scaled(_)) => Value::Scaled(sva.raw()),
        (CommandValue::SetPointFloat { r32, .. }, Value::Float(_)) => Value::Float(r32.raw()),
        (CommandValue::Bitstring { bsi }, Value::Bitstring(_)) => Value::Bitstring(bsi.bits),
        _ => return None,
    })
}

pub struct Simulator {
    runtime: Runtime,
    server: Server,
}

impl Simulator {
    pub fn start(
        settings: &Settings,
        echo: bool,
        notices: mpsc::Sender<Notice>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let wake: Wake = Arc::new(wake);
        let mut config = ServerConfig::new(settings.common_address);
        config.sizes = settings.sizes()?;
        config.link = settings.link()?;
        if settings.select_before_execute {
            config.command_mode = CommandMode::SelectAndExecute;
        }
        for point in &settings.station {
            let mut pc = PointConfig::new(point.parse()?).with_quality(point.quality());
            if point.group > 0 {
                pc = pc.group(point.group);
            }
            if !point.tagged {
                pc = pc.untagged();
            }
            config = config.point(point.ioa, pc);
            if point.commands
                && let Some(family) = point.kind.family()
            {
                config = config.command(point.ioa, family);
            }
        }
        let tls = settings
            .tls
            .enabled
            .then(|| settings.tls_config())
            .transpose()?;
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .map_err(|e| e.to_string())?;
        let cell = Arc::new(OnceLock::new());
        let station = Station {
            server: Arc::clone(&cell),
            notices: notices.clone(),
            wake: Arc::clone(&wake),
            echo,
        };
        let address = settings.address();
        let (server, listener) = runtime.block_on(async {
            let server = Server::spawn(config, station).map_err(|e| e.to_string())?;
            let listener = TcpListener::bind(&address)
                .await
                .map_err(|e| format!("{address}: {e}"))?;
            Ok::<_, String>((server, listener))
        })?;
        let _ = cell.set(server.clone());
        runtime.spawn(serve(listener, server.clone(), tls, notices, wake));
        Ok(Self { runtime, server })
    }

    pub fn report(&self, point: &SimPoint) -> Result<(), String> {
        let value = point.parse()?;
        let server = self.server.clone();
        let quality = point.quality();
        let ioa = point.ioa;
        self.runtime
            .block_on(async move { server.report(ioa, value, quality, None).await })
            .map_err(|e| e.to_string())
    }

    pub fn shutdown(self) {
        let server = self.server;
        self.runtime
            .block_on(async move { server.shutdown().await });
        self.runtime.shutdown_background();
    }
}

async fn serve(
    listener: TcpListener,
    server: Server,
    tls: Option<TlsConfig>,
    notices: mpsc::Sender<Notice>,
    wake: Wake,
) {
    let log = |line: String| {
        let _ = notices.send(Notice::now(NoticeKind::Log(line)));
        wake();
    };
    loop {
        let Ok((tcp, peer)) = listener.accept().await else {
            continue;
        };
        let _ = tcp.set_nodelay(true);
        let attached = match &tls {
            Some(config) => match iec104_tls::tokio::accept(tcp, config).await {
                Ok(stream) => {
                    server
                        .attach(Tap::new(stream, notices.clone(), Arc::clone(&wake)))
                        .await
                }
                Err(e) => {
                    log(format!("{peer}: TLS handshake failed: {e}"));
                    continue;
                }
            },
            None => {
                server
                    .attach(Tap::new(tcp, notices.clone(), Arc::clone(&wake)))
                    .await
            }
        };
        match attached {
            Ok(_) => log(format!("{peer} connected")),
            Err(e) => log(format!("{peer} refused: {e}")),
        }
    }
}

/// Passes a socket through and reports every whole APDU read or written, as
/// the client's driver does with its trace.
struct Tap<S> {
    inner: S,
    notices: mpsc::Sender<Notice>,
    wake: Wake,
    received: Vec<u8>,
    sent: Vec<u8>,
}

impl<S> Tap<S> {
    const fn new(inner: S, notices: mpsc::Sender<Notice>, wake: Wake) -> Self {
        Self {
            inner,
            notices,
            wake,
            received: Vec::new(),
            sent: Vec::new(),
        }
    }
}

/// Moves every whole APDU out of `buffer`. A start octet other than 68H
/// drops the buffer: the session closes such a connection anyway.
fn frames(buffer: &mut Vec<u8>, bytes: &[u8]) -> Vec<Vec<u8>> {
    buffer.extend_from_slice(bytes);
    let mut out = Vec::new();
    while let (Some(&start), Some(&length)) = (buffer.first(), buffer.get(1)) {
        if start != 0x68 {
            buffer.clear();
            break;
        }
        let total = usize::from(length).saturating_add(2);
        if buffer.len() < total {
            break;
        }
        out.push(buffer.drain(..total).collect());
    }
    out
}

impl<S> Tap<S> {
    fn emit(&self, frames: Vec<Vec<u8>>, sent: bool) {
        for frame in frames {
            let event = if sent {
                Event::Sent { conn: 0, frame }
            } else {
                Event::Received { conn: 0, frame }
            };
            let _ = self.notices.send(Notice::now(NoticeKind::Event(event)));
        }
        (self.wake)();
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Tap<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        if matches!(poll, Poll::Ready(Ok(())))
            && let Some(new) = buf.filled().get(before..)
            && !new.is_empty()
        {
            let this = &mut *self;
            let found = frames(&mut this.received, new);
            this.emit(found, false);
        }
        poll
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Tap<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        let poll = Pin::new(&mut self.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = poll
            && let Some(written) = buf.get(..n)
        {
            let this = &mut *self;
            let found = frames(&mut this.sent, written);
            this.emit(found, true);
        }
        poll
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use iec104::link::State;

    use super::*;
    use crate::commands::{Context as SendContext, Forms, SystemKind, Tab};
    use crate::driver::Driver;
    use crate::points::Points;

    #[test]
    fn select_before_execute() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let settings = Settings {
            port,
            station: vec![SimPoint {
                commands: true,
                ..SimPoint::new(20)
            }],
            select_before_execute: true,
            ..Settings::default()
        };
        let (station_tx, _station_rx) = mpsc::channel();
        let simulator = Simulator::start(&settings, true, station_tx, || {}).unwrap();
        let (tx, rx) = mpsc::channel();
        let driver = Driver::connect(&settings, tx, || {}).unwrap();
        let ctx = SendContext {
            sizes: settings.sizes().unwrap(),
            common_address: settings.common_address,
            originator: 0,
        };
        let mut forms = Forms {
            ioa: "20".into(),
            ..Forms::default()
        };
        let direct = forms.build(&ctx).unwrap();
        let Ok(crate::commands::Action::SelectExecute {
            select,
            execute,
            type_id,
            ioa,
        }) = forms.select_execute(&ctx)
        else {
            panic!()
        };

        let mut confirmations = Vec::new();
        let mut returned = false;
        let deadline = Instant::now() + Duration::from_secs(10);
        while !returned && Instant::now() < deadline {
            let Ok(notice) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            match notice.kind {
                NoticeKind::Event(Event::State {
                    state: State::Started,
                    ..
                }) => {
                    driver.send(direct.clone());
                }
                NoticeKind::Event(Event::Asdu { asdu, .. }) => {
                    if let Some(positive) =
                        crate::commands::confirmation(&asdu, ctx.sizes, type_id, ioa)
                    {
                        confirmations.push(positive);
                        match confirmations.len() {
                            1 => driver.send(select.clone()),
                            2 => driver.send(execute.clone()),
                            _ => {}
                        }
                    }
                    let mut points = Points::default();
                    points.apply(&asdu, ctx.sizes, notice.at);
                    returned = points
                        .rows
                        .get(&(1, 20))
                        .is_some_and(|r| r.cause == "retrem");
                }
                _ => {}
            }
        }
        driver.shutdown();
        simulator.shutdown();
        assert_eq!(confirmations, [false, true, true]);
        assert!(returned);
    }

    #[test]
    fn client_talks_to_simulator() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let settings = Settings {
            port,
            station: vec![
                SimPoint {
                    value: "1".into(),
                    commands: true,
                    ..SimPoint::new(10)
                },
                SimPoint {
                    kind: Kind::Scaled,
                    value: "-7".into(),
                    group: 1,
                    ..SimPoint::new(11)
                },
            ],
            ..Settings::default()
        };
        let (station_tx, station_rx) = mpsc::channel();
        let simulator = Simulator::start(&settings, true, station_tx, || {}).unwrap();
        let (tx, rx) = mpsc::channel();
        let driver = Driver::connect(&settings, tx, || {}).unwrap();
        let ctx = SendContext {
            sizes: settings.sizes().unwrap(),
            common_address: settings.common_address,
            originator: 0,
        };
        let mut gi = Forms {
            tab: Tab::System,
            system: SystemKind::Interrogation,
            qualifier: "20".into(),
            ..Forms::default()
        };
        let mut off = Forms {
            ioa: "10".into(),
            on: false,
            ..Forms::default()
        };

        let mut points = Points::default();
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut step = 0;
        while Instant::now() < deadline {
            let Ok(notice) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            match notice.kind {
                NoticeKind::Event(Event::State {
                    state: State::Started,
                    ..
                }) if step == 0 => {
                    driver.send(gi.build(&ctx).unwrap());
                    step = 1;
                }
                NoticeKind::Event(Event::Asdu { asdu, .. }) => {
                    points.apply(&asdu, ctx.sizes, notice.at);
                    if step == 1 && points.rows.len() == 2 {
                        driver.send(off.build(&ctx).unwrap());
                        step = 2;
                    }
                    if step == 2 && points.rows[&(1, 10)].cause == "retrem" {
                        break;
                    }
                }
                _ => {}
            }
        }
        driver.shutdown();
        let tapped: Vec<_> = station_rx.try_iter().collect();
        simulator.shutdown();
        assert_eq!(points.rows[&(1, 11)].value, "-7");
        assert_eq!(points.rows[&(1, 10)].value, "OFF");
        assert_eq!(points.rows[&(1, 10)].cause, "retrem");
        assert!(
            tapped
                .iter()
                .any(|n| matches!(&n.kind, NoticeKind::Event(Event::Received { .. })))
        );
        assert!(tapped.iter().any(|n| matches!(
            &n.kind,
            NoticeKind::Echo {
                ioa: 10,
                value: Value::Single(false)
            }
        )));
    }

    #[test]
    fn splits_frames_across_reads() {
        let mut buffer = Vec::new();
        assert_eq!(
            frames(&mut buffer, &[0x68, 0x04, 0x07]),
            Vec::<Vec<u8>>::new()
        );
        let got = frames(
            &mut buffer,
            &[0x00, 0x00, 0x00, 0x68, 0x04, 0x0B, 0x00, 0x00, 0x00, 0x68],
        );
        assert_eq!(
            got,
            [
                vec![0x68, 0x04, 0x07, 0, 0, 0],
                vec![0x68, 0x04, 0x0B, 0, 0, 0]
            ]
        );
        assert_eq!(buffer, [0x68]);
    }

    #[test]
    fn values_round_trip_through_text() {
        for (kind, text) in [
            (Kind::Single, "1"),
            (Kind::Double, "2"),
            (Kind::Step, "-5"),
            (Kind::Bitstring, "0000BEEF"),
            (Kind::Normalized, "0.5"),
            (Kind::Scaled, "-1234"),
            (Kind::Float, "1.5"),
            (Kind::Counter, "77"),
        ] {
            let point = SimPoint {
                kind,
                value: text.into(),
                ..SimPoint::new(1)
            };
            assert_eq!(value_text(&point.parse().unwrap()), text, "{kind:?}");
        }
        assert!(
            SimPoint {
                value: "2".into(),
                ..SimPoint::new(1)
            }
            .parse()
            .is_err()
        );
    }
}
