use std::io;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::mpsc;
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use iec104::asdu::{
    Cause, DoubleCommandState, DoublePointState, Nof, Nva, Point, Qds, Quality, Value,
};
use iec104::ct::{Command, CommandMode, CommandValue, Family};
use iec104_tls::TlsConfig;
use iec104_tokio::Event;
use iec104_tokio::server::{Application, PointConfig, Server, ServerConfig};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::runtime::Runtime;

use crate::commands::{Forms, MonitorKind, Tab};
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

    /// Whether the element's quality has an `OV` bit.
    pub const fn overflows(self) -> bool {
        matches!(
            self,
            Self::Step | Self::Bitstring | Self::Normalized | Self::Scaled | Self::Float
        )
    }
}

/// A point of the simulated station. `value` is text so the table can edit it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "one per quality flag and option"
)]
pub struct SimPoint {
    pub ioa: u32,
    pub kind: Kind,
    pub value: String,
    pub invalid: bool,
    #[serde(default)]
    pub not_topical: bool,
    #[serde(default)]
    pub substituted: bool,
    #[serde(default)]
    pub blocked: bool,
    #[serde(default)]
    pub overflow: bool,
    /// Seconds between cyclic sends of the point (cause 1), 0 for none.
    #[serde(default)]
    pub period: u32,
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
            not_topical: false,
            substituted: false,
            blocked: false,
            overflow: false,
            period: 0,
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
            Kind::Bitstring => Value::Bitstring(crate::commands::hex_u32(text).map_err(|_| bad())?),
            Kind::Normalized => Value::Normalized(
                crate::commands::normalized(text)
                    .map_err(|_| bad())?
                    .to_scaled(),
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
            not_topical: self.not_topical,
            substituted: self.substituted,
            blocked: self.blocked,
            overflow: self.kind.overflows().then_some(self.overflow),
        }
    }
}

/// A file the simulator serves, read from `path` when it starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServedFile {
    pub ioa: u32,
    pub nof: u16,
    pub path: String,
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
    upload_folder: std::path::PathBuf,
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

    fn received_file(&self, ioa: u32, nof: Nof, content: Vec<u8>) {
        let path = self
            .upload_folder
            .join(format!("upload-{ioa}-{}.bin", nof.value));
        let line = match std::fs::write(&path, &content) {
            Ok(()) => format!(
                "upload IOA {ioa} name {}: {} octets saved to {}",
                nof.value,
                content.len(),
                path.display()
            ),
            Err(e) => format!(
                "upload IOA {ioa} name {}: {}: {e}",
                nof.value,
                path.display()
            ),
        };
        self.log(line);
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
        notices: mpsc::Sender<Notice>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let wake: Wake = Arc::new(wake);
        let mut config = ServerConfig::new(settings.common_address);
        let config_sizes = settings.sizes()?;
        config.sizes = config_sizes;
        config.link = settings.link()?;
        config.accept_uploads = settings.accept_uploads;
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
        for file in &settings.served {
            let content = std::fs::read(&file.path).map_err(|e| format!("{}: {e}", file.path))?;
            config = config.file(file.ioa, Nof { value: file.nof }, content);
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
            echo: settings.echo,
            upload_folder: settings.upload_folder.clone().into(),
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
        for point in settings.station.iter().filter(|p| p.period > 0) {
            let ctx = crate::commands::Context {
                sizes: config_sizes,
                common_address: settings.common_address,
                originator: 0,
            };
            let period = Duration::from_secs(point.period.into());
            runtime.spawn(cyclic(server.clone(), point.ioa, period, ctx));
        }
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

/// Sends the point as the server holds it, so a reported edit goes out too.
/// `Server::report_with` takes event causes only, so the ASDU goes to the
/// driver underneath, which puts it on the started connection.
async fn cyclic(server: Server, ioa: u32, period: Duration, ctx: crate::commands::Context) {
    let mut ticks = tokio::time::interval(period);
    ticks.tick().await;
    loop {
        ticks.tick().await;
        let Ok(point) = server.point(ioa).await else {
            return;
        };
        // Refused while no client has started data transfer.
        if let Ok(asdu) = cyclic_asdu(&ctx, ioa, &point) {
            let _ = server.handle().send_asdu(0, asdu).await;
        }
    }
}

fn cyclic_asdu(ctx: &crate::commands::Context, ioa: u32, point: &Point) -> Result<Vec<u8>, String> {
    let q = point.quality.unwrap_or_default();
    let mut forms = Forms {
        tab: Tab::Monitor,
        cause: Cause::Periodic.to_u8(),
        ioa: ioa.to_string(),
        value: value_text(&point.value),
        quality: Qds {
            overflow: q.overflow.unwrap_or(false),
            blocked: q.blocked,
            substituted: q.substituted,
            not_topical: q.not_topical,
            invalid: q.invalid,
        },
        ..Forms::default()
    };
    forms.monitor = match point.value {
        Value::Single(on) => {
            forms.on = on;
            MonitorKind::Single
        }
        Value::Double(state) => {
            forms.double = state;
            MonitorKind::Double
        }
        Value::StepPosition { .. } => MonitorKind::Step,
        Value::Bitstring(_) => MonitorKind::Bitstring,
        Value::Normalized(_) => MonitorKind::Normalized,
        Value::Scaled(_) => MonitorKind::Scaled,
        Value::Float(_) => MonitorKind::Float,
        Value::Counter { .. } => MonitorKind::Counter,
        other => return Err(format!("{other:?}")),
    };
    forms.build(ctx)
}

async fn serve(
    listener: TcpListener,
    server: Server,
    tls: Option<TlsConfig>,
    notices: mpsc::Sender<Notice>,
    wake: Wake,
) {
    loop {
        let Ok((tcp, peer)) = listener.accept().await else {
            // A lasting error (out of file descriptors) would otherwise spin.
            tokio::time::sleep(Duration::from_millis(100)).await;
            continue;
        };
        let _ = tcp.set_nodelay(true);
        // Each in its own task: a client that stalls its TLS handshake must
        // not hold off the next connection.
        tokio::spawn(admit(
            tcp,
            peer,
            server.clone(),
            tls.clone(),
            notices.clone(),
            Arc::clone(&wake),
        ));
    }
}

async fn admit(
    tcp: TcpStream,
    peer: SocketAddr,
    server: Server,
    tls: Option<TlsConfig>,
    notices: mpsc::Sender<Notice>,
    wake: Wake,
) {
    let log = |line: String| {
        let _ = notices.send(Notice::now(NoticeKind::Log(line)));
        wake();
    };
    let attached = match &tls {
        Some(config) => match iec104_tls::tokio::accept(tcp, config).await {
            Ok(stream) => {
                server
                    .attach(Tap::new(stream, peer, notices.clone(), Arc::clone(&wake)))
                    .await
            }
            Err(e) => return log(format!("{peer}: TLS handshake failed: {e}")),
        },
        None => {
            server
                .attach(Tap::new(tcp, peer, notices.clone(), Arc::clone(&wake)))
                .await
        }
    };
    match attached {
        Ok(_) => log(format!("{peer} connected")),
        Err(e) => log(format!("{peer} refused: {e}")),
    }
}

/// Passes a socket through and reports every whole APDU read or written, as
/// the client's driver does with its trace, and logs the client leaving.
struct Tap<S> {
    inner: S,
    peer: SocketAddr,
    gone: bool,
    notices: mpsc::Sender<Notice>,
    wake: Wake,
    received: Vec<u8>,
    sent: Vec<u8>,
}

impl<S> Tap<S> {
    const fn new(inner: S, peer: SocketAddr, notices: mpsc::Sender<Notice>, wake: Wake) -> Self {
        Self {
            inner,
            peer,
            gone: false,
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

    fn leave(&mut self) {
        if !self.gone {
            self.gone = true;
            let line = format!("{} disconnected", self.peer);
            let _ = self.notices.send(Notice::now(NoticeKind::Log(line)));
            (self.wake)();
        }
    }
}

// The session may close the socket without reading it to the end.
impl<S> Drop for Tap<S> {
    fn drop(&mut self) {
        self.leave();
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for Tap<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let before = buf.filled().len();
        let room = buf.remaining() > 0;
        let poll = Pin::new(&mut self.inner).poll_read(cx, buf);
        let this = &mut *self;
        match &poll {
            Poll::Ready(Ok(())) => match buf.filled().get(before..) {
                Some(new) if !new.is_empty() => {
                    let found = frames(&mut this.received, new);
                    this.emit(found, false);
                }
                _ if room => this.leave(),
                _ => {}
            },
            Poll::Ready(Err(_)) => this.leave(),
            Poll::Pending => {}
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
    use crate::settings::tests::{test_pki, tls_settings};

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
        let simulator = Simulator::start(&settings, station_tx, || {}).unwrap();
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
    fn serves_files_to_the_client() {
        use crate::files::Files;

        let content: Vec<u8> = (0..3000u16)
            .map(|i| u8::try_from(i % 251).unwrap())
            .collect();
        let source = std::env::temp_dir().join("iec104-workbench-served.bin");
        std::fs::write(&source, &content).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let settings = Settings {
            port,
            served: vec![ServedFile {
                ioa: 30000,
                nof: 4,
                path: source.display().to_string(),
            }],
            ..Settings::default()
        };
        let (station_tx, _station_rx) = mpsc::channel();
        let simulator = Simulator::start(&settings, station_tx, || {}).unwrap();
        let (tx, rx) = mpsc::channel();
        let driver = Driver::connect(&settings, tx, || {}).unwrap();
        let ctx = SendContext {
            sizes: settings.sizes().unwrap(),
            common_address: settings.common_address,
            originator: 0,
        };
        let target = std::env::temp_dir().join("iec104-workbench-fetched.bin");
        let mut files = Files::default();
        let mut step = 0;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && step < 3 {
            let Ok(notice) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            match notice.kind {
                NoticeKind::Event(Event::State {
                    state: State::Started,
                    ..
                }) if step == 0 => {
                    driver.send(files.call_directory(&ctx).unwrap());
                    step = 1;
                }
                NoticeKind::Event(Event::Asdu { asdu, .. }) => {
                    for reply in files.on_asdu(&asdu) {
                        driver.send(reply);
                    }
                    if step == 1 && !files.busy() {
                        let listed = files.listing.first().unwrap();
                        assert_eq!((listed.ioa, listed.entry.nof.value), (30000, 4));
                        driver.send(files.download(&ctx, 30000, 4, target.clone()).unwrap());
                        step = 2;
                    } else if step == 2 && !files.busy() {
                        step = 3;
                    }
                }
                _ => {}
            }
        }
        driver.shutdown();
        simulator.shutdown();
        let fetched = std::fs::read(&target).unwrap();
        let _ = std::fs::remove_file(&source);
        let _ = std::fs::remove_file(&target);
        assert_eq!(fetched, content, "{}", files.status);
    }

    #[test]
    fn receives_uploads_from_the_client() {
        use crate::files::Files;

        let content: Vec<u8> = (0..3000u16)
            .map(|i| u8::try_from(i % 241).unwrap())
            .collect();
        let folder =
            std::env::temp_dir().join(format!("iec104-workbench-uploads-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let settings = Settings {
            port,
            accept_uploads: true,
            upload_folder: folder.display().to_string(),
            ..Settings::default()
        };
        let (station_tx, station_rx) = mpsc::channel();
        let simulator = Simulator::start(&settings, station_tx, || {}).unwrap();
        let (tx, rx) = mpsc::channel();
        let driver = Driver::connect(&settings, tx, || {}).unwrap();
        let ctx = SendContext {
            sizes: settings.sizes().unwrap(),
            common_address: settings.common_address,
            originator: 0,
        };
        let mut files = Files::default();
        let mut step = 0;
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline && step < 2 {
            let Ok(notice) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            match notice.kind {
                NoticeKind::Event(Event::State {
                    state: State::Started,
                    ..
                }) if step == 0 => {
                    driver.send(files.upload(&ctx, 40000, 5, content.clone()).unwrap());
                    step = 1;
                }
                NoticeKind::Event(Event::Asdu { asdu, .. }) => {
                    for reply in files.on_asdu(&asdu) {
                        driver.send(reply);
                    }
                    if step == 1 && !files.busy() {
                        step = 2;
                    }
                }
                _ => {}
            }
        }
        driver.shutdown();
        let logged: Vec<_> = station_rx.try_iter().collect();
        simulator.shutdown();
        let saved = std::fs::read(folder.join("upload-40000-5.bin"));
        let _ = std::fs::remove_dir_all(&folder);
        assert_eq!(saved.unwrap(), content, "{}", files.status);
        assert!(logged.iter().any(
            |n| matches!(&n.kind, NoticeKind::Log(line) if line.starts_with("upload IOA 40000 name 5: 3000 octets"))
        ));
    }

    /// Mutual TLS with a throwaway CA, and a first client that connects but
    /// never starts its handshake: the second must not wait for the first's
    /// handshake timeout (10 s).
    #[test]
    fn a_stalled_tls_handshake_does_not_block_the_next_client() {
        let pki = test_pki("tls-stall");
        let settings = tls_settings(&pki, "a", free_port());
        let (station_tx, _station_rx) = mpsc::channel();
        let simulator = Simulator::start(&settings, station_tx, || {}).unwrap();
        let _stalled = std::net::TcpStream::connect(("127.0.0.1", settings.port)).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        let started = client_starts(&settings, Duration::from_secs(5));
        simulator.shutdown();
        let _ = std::fs::remove_dir_all(&pki);
        assert!(started, "the TLS client started data transfer within 5 s");
    }

    #[test]
    fn a_peer_off_the_allow_list_is_refused() {
        let pki = test_pki("tls-allow");
        let mut station = tls_settings(&pki, "a", free_port());
        station.tls.allowed_peers = pki.join("a.pem").display().to_string();
        let (station_tx, _station_rx) = mpsc::channel();
        let simulator = Simulator::start(&station, station_tx, || {}).unwrap();
        let allowed = client_starts(&station, Duration::from_secs(5));
        let other = client_starts(
            &tls_settings(&pki, "b", station.port),
            Duration::from_secs(2),
        );
        simulator.shutdown();
        let _ = std::fs::remove_dir_all(&pki);
        assert!(allowed, "the listed peer started data transfer");
        assert!(!other, "the unlisted peer was refused");
    }

    #[test]
    fn a_revoked_peer_is_refused() {
        let pki = test_pki("tls-crl");
        let mut station = tls_settings(&pki, "a", free_port());
        station.tls.crl = pki.join("crl.pem").display().to_string();
        let (station_tx, _station_rx) = mpsc::channel();
        let simulator = Simulator::start(&station, station_tx, || {}).unwrap();
        let valid = client_starts(&station, Duration::from_secs(5));
        let revoked = client_starts(
            &tls_settings(&pki, "b", station.port),
            Duration::from_secs(2),
        );
        simulator.shutdown();
        let _ = std::fs::remove_dir_all(&pki);
        assert!(valid, "the valid peer started data transfer");
        assert!(!revoked, "the revoked peer was refused");
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    fn client_starts(settings: &Settings, within: Duration) -> bool {
        let began = Instant::now();
        let (tx, rx) = mpsc::channel();
        let driver = Driver::connect(settings, tx, || {}).unwrap();
        let mut started = false;
        while !started && began.elapsed() < within {
            if let Ok(Notice {
                kind:
                    NoticeKind::Event(Event::State {
                        state: State::Started,
                        ..
                    }),
                ..
            }) = rx.recv_timeout(Duration::from_millis(100))
            {
                started = true;
            }
        }
        driver.shutdown();
        started
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
        let simulator = Simulator::start(&settings, station_tx, || {}).unwrap();
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

        let flagged = flagged(settings.station[1].clone());

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
                        simulator.report(&flagged).unwrap();
                        step = 3;
                    }
                    if step == 3 && points.rows[&(1, 11)].cause == "spont" {
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
        assert_eq!(points.rows[&(1, 11)].quality, "IV NT SB BL OV");
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

    fn flagged(point: SimPoint) -> SimPoint {
        SimPoint {
            invalid: true,
            not_topical: true,
            substituted: true,
            blocked: true,
            overflow: true,
            ..point
        }
    }

    #[test]
    fn quality_maps_every_flag() {
        let flagged = |kind| {
            flagged(SimPoint {
                kind,
                ..SimPoint::new(1)
            })
        };
        assert_eq!(
            flagged(Kind::Float).quality(),
            Quality {
                invalid: true,
                not_topical: true,
                substituted: true,
                blocked: true,
                overflow: Some(true),
            }
        );
        assert_eq!(flagged(Kind::Single).quality().overflow, None);
        assert_eq!(SimPoint::new(1).quality(), Quality::default());
    }

    #[test]
    fn logs_a_client_leaving_once() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let settings = Settings {
            port,
            ..Settings::default()
        };
        let (station_tx, station_rx) = mpsc::channel();
        let simulator = Simulator::start(&settings, station_tx, || {}).unwrap();
        let client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        let peer = client.local_addr().unwrap();
        let mut lines = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Notice {
                kind: NoticeKind::Log(line),
                ..
            }) = station_rx.recv_timeout(Duration::from_millis(100))
            {
                if line.ends_with(" connected") {
                    client.shutdown(std::net::Shutdown::Both).unwrap();
                }
                lines.push(line);
                if lines.len() == 2 {
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(200));
        simulator.shutdown();
        lines.extend(station_rx.try_iter().filter_map(|n| match n.kind {
            NoticeKind::Log(line) => Some(line),
            _ => None,
        }));
        assert_eq!(
            lines,
            [format!("{peer} connected"), format!("{peer} disconnected")]
        );
    }

    #[test]
    fn sends_the_current_value_cyclically() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let point = SimPoint {
            kind: Kind::Scaled,
            value: "-7".into(),
            period: 1,
            ..SimPoint::new(12)
        };
        let settings = Settings {
            port,
            station: vec![point.clone()],
            ..Settings::default()
        };
        let (station_tx, _station_rx) = mpsc::channel();
        let simulator = Simulator::start(&settings, station_tx, || {}).unwrap();
        let (tx, rx) = mpsc::channel();
        let driver = Driver::connect(&settings, tx, || {}).unwrap();
        let sizes = settings.sizes().unwrap();
        let periodic = crate::telegram::cause_name(Cause::Periodic);
        let mut points = Points::default();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let Ok(notice) = rx.recv_timeout(Duration::from_millis(100)) else {
                continue;
            };
            match notice.kind {
                NoticeKind::Event(Event::State {
                    state: State::Started,
                    ..
                }) => {
                    let edited = SimPoint {
                        value: "5".into(),
                        ..point.clone()
                    };
                    simulator.report(&edited).unwrap();
                }
                NoticeKind::Event(Event::Asdu { asdu, .. }) => {
                    points.apply(&asdu, sizes, notice.at);
                    if points
                        .rows
                        .get(&(1, 12))
                        .is_some_and(|r| r.cause == periodic)
                    {
                        break;
                    }
                }
                _ => {}
            }
        }
        driver.shutdown();
        simulator.shutdown();
        assert_eq!(points.rows[&(1, 12)].cause, periodic);
        assert_eq!(points.rows[&(1, 12)].value, "5");
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
        // The send forms' rule: `0x` or `0X`, either case of digit.
        for text in ["0xbeef", "0XBEEF", "beef"] {
            let point = SimPoint {
                kind: Kind::Bitstring,
                value: text.into(),
                ..SimPoint::new(1)
            };
            assert_eq!(point.parse(), Ok(Value::Bitstring(0xBEEF)), "{text}");
        }
    }
}
