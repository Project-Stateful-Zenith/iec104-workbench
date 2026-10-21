use std::cell::RefCell;
use std::fs::File;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::Local;
use iec104::asdu::{self, AddressSizes, Header};
use iec104::link::State;
use iec104_tokio::Event;
use rhai::{Array, Dynamic, Engine, EvalAltResult, Map};

use crate::commands::{Context, Forms, ProcessKind, SystemKind, Tab};
use crate::driver::{Driver, Notice, NoticeKind, state_name};
use crate::points::{Points, quality_text, value_text};
use crate::settings::Settings;
use crate::telegram::{Dir, MAX_TELEGRAMS, Telegram, make_room, parse_hex};

type Fail = Box<EvalAltResult>;

const USAGE: &str = "usage: iec104-workbench run <script.rhai> [--profile <profile.toml>] \
                     [--log <file>] [--log-level all|info|warn|error|none] [--record <file>]";

/// Received ASDUs that `wait_for` has not taken yet.
const MAX_RECEIVED: usize = 10_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    All,
    Info,
    Warn,
    Error,
    None,
}

#[derive(Debug, PartialEq, Eq)]
struct Args {
    script: PathBuf,
    profile: Option<PathBuf>,
    log: Option<PathBuf>,
    level: Level,
    record: Option<PathBuf>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<Args, String> {
    let script = args.next().ok_or("no script given")?;
    let mut parsed = Args {
        script: script.into(),
        profile: None,
        log: None,
        level: Level::Info,
        record: None,
    };
    while let Some(flag) = args.next() {
        let value = args.next().ok_or_else(|| format!("{flag} needs a value"))?;
        match flag.as_str() {
            "--profile" => parsed.profile = Some(value.into()),
            "--log" => parsed.log = Some(value.into()),
            "--record" => parsed.record = Some(value.into()),
            "--log-level" => {
                parsed.level = match value.as_str() {
                    "all" => Level::All,
                    "info" => Level::Info,
                    "warn" => Level::Warn,
                    "error" => Level::Error,
                    "none" => Level::None,
                    _ => return Err(format!("unknown log level {value}")),
                };
            }
            _ => return Err(format!("unknown option {flag}")),
        }
    }
    Ok(parsed)
}

/// `iec104-workbench run ...`: 0 when the script passes, 1 when it fails, 2
/// when it cannot start.
pub fn main(args: impl Iterator<Item = String>) -> ExitCode {
    let started = parse_args(args).and_then(|args| {
        let text = std::fs::read_to_string(&args.script)
            .map_err(|e| format!("{}: {e}", args.script.display()))?;
        let settings = match &args.profile {
            Some(path) => Settings::load(path).map_err(|e| format!("{}: {e}", path.display()))?,
            None => Settings::default(),
        };
        let file = args
            .log
            .as_ref()
            .map(|path| File::create(path).map_err(|e| format!("{}: {e}", path.display())))
            .transpose()?;
        Ok((args, text, settings, file))
    });
    let (args, text, settings, file) = match started {
        Ok(started) => started,
        Err(e) => {
            eprintln!("{e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    let mut log = Log {
        level: args.level,
        file,
    };
    let (result, telegrams) = run(&text, settings, &mut log);
    if let Some(path) = &args.record
        && let Err(e) = crate::telegram::save(path, &telegrams)
    {
        log.line(Level::Error, &format!("{}: {e}", path.display()));
    }
    match result {
        Ok(()) => {
            log.line(Level::Info, "passed");
            ExitCode::SUCCESS
        }
        Err(e) => {
            log.line(Level::Error, &format!("failed: {e}"));
            ExitCode::FAILURE
        }
    }
}

pub struct Log {
    pub level: Level,
    pub file: Option<File>,
}

impl Log {
    pub const fn quiet() -> Self {
        Self {
            level: Level::None,
            file: None,
        }
    }

    fn line(&mut self, level: Level, text: &str) {
        if level < self.level {
            return;
        }
        let line = format!("{} {level:?} {text}", Local::now().format("%T%.3f"));
        eprintln!("{line}");
        if let Some(file) = &mut self.file {
            let _ = writeln!(file, "{line}");
        }
    }
}

/// Runs `script` and returns its outcome and every telegram of the run.
pub fn run(script: &str, settings: Settings, log: &mut Log) -> (Result<(), String>, Vec<Telegram>) {
    let sizes = match settings.sizes() {
        Ok(sizes) => sizes,
        Err(e) => return (Err(e), Vec::new()),
    };
    let (sender, notices) = mpsc::channel();
    let session = Rc::new(RefCell::new(Session {
        settings,
        sizes,
        driver: None,
        sender,
        notices,
        state: None,
        connected: false,
        failed: None,
        received: Vec::new(),
        points: Points::default(),
        telegrams: Vec::new(),
        log: std::mem::replace(log, Log::quiet()),
    }));
    let result = engine(&session).run(script).map_err(|e| e.to_string());
    let mut session = session.borrow_mut();
    session.disconnect();
    std::mem::swap(log, &mut session.log);
    (result, std::mem::take(&mut session.telegrams))
}

fn engine(session: &Rc<RefCell<Session>>) -> Engine {
    let mut engine = Engine::new();
    let s = Rc::clone(session);
    engine.register_fn("connect", move || s.borrow_mut().connect());
    let s = Rc::clone(session);
    engine.register_fn("disconnect", move || s.borrow_mut().disconnect());
    let s = Rc::clone(session);
    engine.register_fn("start_dt", move || s.borrow_mut().start_dt());
    let s = Rc::clone(session);
    engine.register_fn("stop_dt", move || s.borrow_mut().stop_dt());
    let s = Rc::clone(session);
    engine.register_fn("send_hex", move |text: &str| {
        let asdu = parse_hex(text)?;
        s.borrow_mut().send(asdu)
    });
    let s = Rc::clone(session);
    engine.register_fn("interrogation", move |qoi: i64| {
        s.borrow_mut().send_form(|f| {
            f.tab = Tab::System;
            f.qualifier = qoi.to_string();
        })
    });
    let s = Rc::clone(session);
    engine.register_fn("counter_interrogation", move |rqt: i64| {
        s.borrow_mut().send_form(|f| {
            f.tab = Tab::System;
            f.system = SystemKind::CounterInterrogation;
            f.qualifier = rqt.to_string();
        })
    });
    let s = Rc::clone(session);
    engine.register_fn("read", move |ioa: i64| {
        s.borrow_mut().send_form(|f| {
            f.tab = Tab::System;
            f.system = SystemKind::Read;
            f.ioa = ioa.to_string();
        })
    });
    let s = Rc::clone(session);
    engine.register_fn("clock_sync", move || {
        s.borrow_mut().send_form(|f| {
            f.tab = Tab::System;
            f.system = SystemKind::ClockSync;
        })
    });
    let s = Rc::clone(session);
    engine.register_fn("single_command", move |ioa: i64, on: bool, select: bool| {
        s.borrow_mut()
            .send_form(|f| command(f, ProcessKind::Single, ioa, on, select))
    });
    let s = Rc::clone(session);
    engine.register_fn("double_command", move |ioa: i64, on: bool, select: bool| {
        s.borrow_mut()
            .send_form(|f| command(f, ProcessKind::Double, ioa, on, select))
    });
    let s = Rc::clone(session);
    engine.register_fn("setpoint_scaled", move |ioa: i64, value: i64| {
        s.borrow_mut()
            .send_form(|f| setpoint(f, ProcessKind::Scaled, ioa, value.to_string()))
    });
    let s = Rc::clone(session);
    engine.register_fn("setpoint_float", move |ioa: i64, value: f64| {
        s.borrow_mut()
            .send_form(|f| setpoint(f, ProcessKind::Float, ioa, value.to_string()))
    });
    let s = Rc::clone(session);
    engine.register_fn(
        "wait_for",
        move |type_id: i64, cause: i64, timeout_ms: i64| {
            s.borrow_mut().wait_for(type_id, cause, timeout_ms)
        },
    );
    let s = Rc::clone(session);
    engine.register_fn("sleep", move |ms: i64| -> Result<(), Fail> {
        s.borrow_mut().wait(millis(ms)?, |_| None::<()>);
        Ok(())
    });
    let s = Rc::clone(session);
    engine.register_fn("point", move |ca: i64, ioa: i64| {
        s.borrow_mut().point(ca, ioa)
    });
    let s = Rc::clone(session);
    engine.register_fn("log", move |text: &str| {
        s.borrow_mut().log.line(Level::Info, text);
    });
    engine.register_fn("expect", |ok: bool, message: &str| -> Result<(), Fail> {
        if ok { Ok(()) } else { Err(message.into()) }
    });
    engine
}

fn command(forms: &mut Forms, kind: ProcessKind, ioa: i64, on: bool, select: bool) {
    forms.process = kind;
    forms.ioa = ioa.to_string();
    forms.on = on;
    forms.select = select;
}

fn setpoint(forms: &mut Forms, kind: ProcessKind, ioa: i64, value: String) {
    forms.process = kind;
    forms.ioa = ioa.to_string();
    forms.value = value;
}

fn millis(ms: i64) -> Result<Duration, Fail> {
    u64::try_from(ms)
        .map(Duration::from_millis)
        .map_err(|_| format!("negative time: {ms} ms").into())
}

struct Session {
    settings: Settings,
    sizes: AddressSizes,
    driver: Option<Driver>,
    sender: mpsc::Sender<Notice>,
    notices: mpsc::Receiver<Notice>,
    state: Option<State>,
    connected: bool,
    failed: Option<String>,
    received: Vec<Vec<u8>>,
    points: Points,
    telegrams: Vec<Telegram>,
    log: Log,
}

impl Session {
    fn connect(&mut self) -> Result<(), Fail> {
        if self.driver.is_some() {
            return Err("already connected".into());
        }
        self.failed = None;
        let address = self.settings.address();
        self.log
            .line(Level::Info, &format!("connecting to {address}"));
        self.driver = Some(Driver::connect(&self.settings, self.sender.clone(), || {})?);
        let started = self.settings.auto_start;
        let timeout = Duration::from_secs(self.settings.t0.saturating_add(self.settings.t1));
        let outcome = self.wait(timeout, |s| {
            s.failed.take().map(Err).or_else(|| {
                let ready = if started {
                    s.state == Some(State::Started)
                } else {
                    s.connected
                };
                ready.then_some(Ok(()))
            })
        });
        let outcome = outcome.unwrap_or_else(|| Err(format!("no connection to {address}")));
        if outcome.is_err() {
            self.disconnect();
        }
        Ok(outcome?)
    }

    fn disconnect(&mut self) {
        if let Some(driver) = self.driver.take() {
            driver.shutdown();
            while let Ok(notice) = self.notices.try_recv() {
                self.handle(notice);
            }
            self.state = None;
            self.connected = false;
        }
    }

    fn start_dt(&mut self) -> Result<(), Fail> {
        self.driver()?.start();
        self.wait_state(State::Started)
    }

    fn stop_dt(&mut self) -> Result<(), Fail> {
        self.driver()?.stop();
        self.wait_state(State::Stopped)
    }

    fn wait_state(&mut self, state: State) -> Result<(), Fail> {
        self.wait(Duration::from_secs(self.settings.t1), |s| {
            (s.state == Some(state)).then_some(())
        })
        .ok_or_else(|| format!("not {} within t1", state_name(state)).into())
    }

    fn driver(&self) -> Result<&Driver, Fail> {
        self.driver.as_ref().ok_or_else(|| "not connected".into())
    }

    fn send(&self, asdu: Vec<u8>) -> Result<(), Fail> {
        self.driver()?.send(asdu);
        Ok(())
    }

    fn send_form(&self, set: impl FnOnce(&mut Forms)) -> Result<(), Fail> {
        let mut forms = Forms::default();
        set(&mut forms);
        let ctx = Context {
            sizes: self.sizes,
            common_address: self.settings.common_address,
            originator: self.settings.originator,
        };
        self.send(forms.build(&ctx)?)
    }

    fn wait_for(&mut self, type_id: i64, cause: i64, timeout_ms: i64) -> Result<Map, Fail> {
        let sizes = self.sizes;
        let found = self.wait(millis(timeout_ms)?, |s| {
            let at = s.received.iter().position(|a| {
                Header::parse_with(a, sizes).is_ok_and(|h| {
                    i64::from(h.type_id.to_u8()) == type_id && i64::from(h.cause.to_u8()) == cause
                })
            })?;
            Some(s.received.remove(at))
        });
        let asdu = found.ok_or_else(|| {
            format!("wait_for({type_id}, {cause}): nothing within {timeout_ms} ms")
        })?;
        asdu_map(&asdu, sizes)
    }

    fn point(&mut self, ca: i64, ioa: i64) -> Result<Dynamic, Fail> {
        let key = (
            u16::try_from(ca).map_err(|_| format!("CA {ca} out of range"))?,
            u32::try_from(ioa).map_err(|_| format!("IOA {ioa} out of range"))?,
        );
        while let Ok(notice) = self.notices.try_recv() {
            self.handle(notice);
        }
        Ok(self
            .points
            .rows
            .get(&key)
            .map_or(Dynamic::UNIT, |row| row.value.clone().into()))
    }

    /// Handles notices until `done` returns a value or `timeout` passes.
    fn wait<T>(
        &mut self,
        timeout: Duration,
        mut done: impl FnMut(&mut Self) -> Option<T>,
    ) -> Option<T> {
        let deadline = Instant::now().checked_add(timeout)?;
        loop {
            if let Some(value) = done(self) {
                return Some(value);
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            if let Ok(notice) = self.notices.recv_timeout(left) {
                self.handle(notice);
            }
        }
    }

    fn handle(&mut self, notice: Notice) {
        let at = notice.at;
        match notice.kind {
            NoticeKind::Event(Event::Sent { frame, .. }) => {
                self.record(Telegram::new(at, Dir::Tx, frame, self.sizes));
            }
            NoticeKind::Event(Event::Received { frame, .. }) => {
                self.record(Telegram::new(at, Dir::Rx, frame, self.sizes));
            }
            NoticeKind::Event(Event::Asdu { asdu, .. }) => {
                self.points.apply(&asdu, self.sizes, at);
                make_room(&mut self.received, MAX_RECEIVED);
                self.received.push(asdu);
            }
            NoticeKind::Event(Event::State { state, .. }) => {
                self.log.line(Level::Info, state_name(state));
                self.state = Some(state);
            }
            NoticeKind::Event(Event::Connected { .. }) => {
                self.log.line(Level::Info, "connected");
                self.connected = true;
            }
            NoticeKind::Event(Event::ConnectFailed { error, .. }) => {
                self.log
                    .line(Level::Warn, &format!("connect failed: {error}"));
                self.failed = Some(error);
            }
            NoticeKind::Event(Event::Disconnected { reason, .. }) => {
                // No driver: the script's own disconnect.
                let level = if self.driver.is_some() {
                    Level::Warn
                } else {
                    Level::Info
                };
                self.log.line(level, &format!("disconnected: {reason}"));
                self.state = None;
                self.connected = false;
            }
            NoticeKind::Error(e) => self.log.line(Level::Warn, &e),
            NoticeKind::Log(line) => self.log.line(Level::Info, &line),
            _ => {}
        }
    }

    fn record(&mut self, telegram: Telegram) {
        self.log.line(
            Level::All,
            &format!("{} {}", telegram.dir.label(), telegram.summary),
        );
        make_room(&mut self.telegrams, MAX_TELEGRAMS);
        self.telegrams.push(telegram);
    }
}

fn asdu_map(asdu: &[u8], sizes: AddressSizes) -> Result<Map, Fail> {
    let decoded = asdu::decode_with(asdu, sizes).map_err(|e| format!("{e:?}"))?;
    let h = decoded.header;
    let objects: Array = decoded
        .objects
        .points()
        .into_iter()
        .map(|p| {
            let mut object = Map::new();
            object.insert("ioa".into(), i64::from(p.ioa).into());
            object.insert("value".into(), value_text(&p.value).into());
            object.insert(
                "quality".into(),
                p.quality.map(quality_text).unwrap_or_default().into(),
            );
            object.into()
        })
        .collect();
    let mut map = Map::new();
    map.insert("type".into(), i64::from(h.type_id.to_u8()).into());
    map.insert("type_name".into(), h.type_id.name().into());
    map.insert("cause".into(), i64::from(h.cause.to_u8()).into());
    map.insert("negative".into(), h.negative.into());
    map.insert("test".into(), h.test.into());
    map.insert("ca".into(), i64::from(h.common_address).into());
    map.insert("objects".into(), objects.into());
    Ok(map)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::driver::tests::station;

    fn run_against_station(script: &str) -> (Result<(), String>, Vec<Telegram>) {
        let (_station, port) = station();
        let settings = Settings {
            port,
            ..Settings::default()
        };
        run(script, settings, &mut Log::quiet())
    }

    #[test]
    fn script_interrogates_a_station() {
        let (result, telegrams) = run_against_station(
            r#"
            connect();
            interrogation(20);
            let con = wait_for(100, 7, 5000);
            expect(!con.negative && con.ca == 1, "interrogation refused");
            let m = wait_for(11, 20, 5000);
            expect(m.objects[0].ioa == 100 && m.objects[0].value == "42", "value " + m.objects[0].value);
            wait_for(100, 10, 5000);
            expect(point(1, 100) == "42", "point");
            expect(point(1, 999) == (), "unknown point");
            disconnect();
            "#,
        );
        assert_eq!(result, Ok(()));
        assert!(telegrams.iter().any(|t| t.dir == Dir::Tx));
        assert!(telegrams.iter().any(|t| t.dir == Dir::Rx));
    }

    #[test]
    fn script_fails_on_a_timed_out_wait() {
        let (result, _) = run_against_station("connect(); wait_for(45, 7, 200);");
        let error = result.unwrap_err();
        assert!(
            error.contains("wait_for(45, 7): nothing within 200 ms"),
            "{error}"
        );
    }

    #[test]
    fn parses_arguments() {
        let args = |text: &str| parse_args(text.split(' ').map(String::from));
        assert_eq!(
            args("t.rhai --log out.log --log-level warn").unwrap(),
            Args {
                script: "t.rhai".into(),
                profile: None,
                log: Some("out.log".into()),
                level: Level::Warn,
                record: None,
            }
        );
        assert!(args("t.rhai --log").is_err());
        assert!(args("t.rhai --log-level loud").is_err());
        assert!(args("t.rhai --verbose x").is_err());
    }
}
