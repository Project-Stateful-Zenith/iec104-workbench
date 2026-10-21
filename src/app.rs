use std::sync::mpsc;
use std::time::{Duration, Instant};

use chrono::Local;
use egui::{Context, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use iec104::link::State;
use iec104_tokio::Event;

use crate::commands::{self, Action, Forms};
use crate::driver::{Driver, Notice, NoticeKind, state_name};
use crate::files::{Files, entry_time};
use crate::points::Points;
use crate::settings::{Settings, TlsVersion};
use crate::simulator::{Kind, ServedFile, SimPoint, Simulator, value_text};
use crate::telegram::{Category, Dir, Filter, MAX_TELEGRAMS, Matcher, Telegram, cause_name, hex};

const SETTINGS_KEY: &str = "settings";

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Telegrams,
    Points,
    Files,
    Station,
}

/// A select sent by "Select + execute", waiting for its confirmation.
struct Pending {
    type_id: u8,
    ioa: u32,
    execute: Vec<u8>,
    sent: Instant,
    timeout: Duration,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Client,
    Simulator,
}

pub struct App {
    settings: Settings,
    mode: Mode,
    driver: Option<Driver>,
    simulator: Option<Simulator>,
    pending: Option<Pending>,
    files: Files,
    log: Vec<String>,
    notices: mpsc::Receiver<Notice>,
    sender: mpsc::Sender<Notice>,
    state: Option<State>,
    window: Option<u16>,
    telegrams: Vec<Telegram>,
    shown: Vec<usize>,
    filter: Filter,
    matcher: Matcher,
    follow: bool,
    selected: Option<usize>,
    open: Vec<usize>,
    points: Points,
    view: View,
    forms: Forms,
    show_settings: bool,
    status: String,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let settings = cc
            .storage
            .and_then(|s| eframe::get_value(s, SETTINGS_KEY))
            .unwrap_or_default();
        let (sender, notices) = mpsc::channel();
        let filter = Filter::default();
        Self {
            settings,
            mode: Mode::Client,
            driver: None,
            simulator: None,
            pending: None,
            files: Files::default(),
            log: Vec::new(),
            notices,
            sender,
            state: None,
            window: None,
            telegrams: Vec::new(),
            shown: Vec::new(),
            matcher: filter.matcher(),
            filter,
            follow: true,
            selected: None,
            open: Vec::new(),
            points: Points::default(),
            view: View::Telegrams,
            forms: Forms::default(),
            show_settings: false,
            status: String::new(),
        }
    }

    /// Settings that shape the frames (address sizes, link parameters, TLS)
    /// must not change under a running connection or station.
    const fn idle(&self) -> bool {
        self.driver.is_none() && self.simulator.is_none()
    }

    fn sizes(&self) -> iec104::asdu::AddressSizes {
        self.settings
            .sizes()
            .unwrap_or(iec104::asdu::AddressSizes::CONVENTIONAL)
    }

    fn push(&mut self, telegram: Telegram) {
        let dropped = crate::telegram::make_room(&mut self.telegrams, MAX_TELEGRAMS);
        if dropped > 0 {
            self.selected = self.selected.and_then(|i| i.checked_sub(dropped));
            self.open = self
                .open
                .iter()
                .filter_map(|i| i.checked_sub(dropped))
                .collect();
            self.refilter();
            self.status = format!("telegram list full: dropped the oldest {dropped}");
        }
        if self.matcher.matches(&telegram) {
            self.shown.push(self.telegrams.len());
        }
        self.telegrams.push(telegram);
    }

    fn refilter(&mut self) {
        self.matcher = self.filter.matcher();
        self.shown = (0..self.telegrams.len())
            .filter(|&i| {
                self.telegrams
                    .get(i)
                    .is_some_and(|t| self.matcher.matches(t))
            })
            .collect();
    }

    fn drain(&mut self) {
        let sizes = self.sizes();
        while let Ok(notice) = self.notices.try_recv() {
            match notice.kind {
                NoticeKind::Event(event) => match event {
                    Event::Sent { frame, .. } => {
                        self.push(Telegram::new(notice.at, Dir::Tx, frame, sizes));
                    }
                    Event::Received { frame, .. } => {
                        self.push(Telegram::new(notice.at, Dir::Rx, frame, sizes));
                    }
                    Event::Asdu { asdu, .. } => {
                        for reply in self.files.on_asdu(&asdu) {
                            if let Some(driver) = &self.driver {
                                driver.send(reply);
                            }
                        }
                        self.confirm(&asdu);
                        self.points.apply(&asdu, sizes, notice.at);
                    }
                    Event::State { state, .. } => self.state = Some(state),
                    Event::Connecting { .. } => {
                        self.status = format!("connecting to {}", self.settings.address());
                    }
                    Event::Connected { .. } => self.status = "connected".into(),
                    Event::ConnectFailed { error, .. } => {
                        self.status = format!("connect failed: {error}");
                    }
                    Event::Disconnected { reason, .. } => {
                        self.state = None;
                        self.window = None;
                        self.status = format!("disconnected: {reason}");
                        // The station does not resume a transfer or a select
                        // on the next connection.
                        self.pending = None;
                        if self.files.busy() {
                            self.files.cancel();
                            self.files.status = format!("stopped, connection lost: {reason}");
                        }
                    }
                    _ => {}
                },
                NoticeKind::Window(room) => self.window = Some(room),
                NoticeKind::Error(e) => self.status = e,
                NoticeKind::Log(line) => {
                    self.log
                        .push(format!("{} {line}", notice.at.format("%T%.3f")));
                }
                NoticeKind::Echo { ioa, value } => {
                    if let Some(point) = self.settings.station.iter_mut().find(|p| p.ioa == ioa) {
                        point.value = value_text(&value);
                    }
                }
            }
        }
    }

    fn confirm(&mut self, asdu: &[u8]) {
        let sizes = self.sizes();
        let Some(pending) = &self.pending else {
            return;
        };
        let Some(positive) = commands::confirmation(asdu, sizes, pending.type_id, pending.ioa)
        else {
            return;
        };
        let ioa = pending.ioa;
        if let Some(pending) = self.pending.take()
            && positive
            && let Some(driver) = &self.driver
        {
            driver.send(pending.execute);
            self.status = format!("IOA {ioa} selected, execute sent");
        } else {
            self.status = format!("IOA {ioa}: select refused by the station, not executed");
        }
    }

    fn act(&mut self, action: Action) {
        let Some(driver) = &self.driver else {
            self.status = "not connected".into();
            return;
        };
        match action {
            Action::Send(asdu) => driver.send(asdu),
            Action::SelectExecute {
                select,
                execute,
                type_id,
                ioa,
            } => {
                driver.send(select);
                self.pending = Some(Pending {
                    type_id,
                    ioa,
                    execute,
                    sent: Instant::now(),
                    timeout: Duration::from_secs(self.settings.t1),
                });
                self.status = format!("IOA {ioa}: select sent, waiting for confirmation");
            }
        }
    }

    fn connect(&mut self, ctx: &Context) {
        let wake = ctx.clone();
        match Driver::connect(&self.settings, self.sender.clone(), move || {
            wake.request_repaint();
        }) {
            Ok(driver) => self.driver = Some(driver),
            Err(e) => self.status = e,
        }
    }

    fn disconnect(&mut self) {
        if let Some(driver) = self.driver.take() {
            driver.shutdown();
        }
        self.state = None;
        self.window = None;
        self.status = "disconnected".into();
    }

    fn start_simulator(&mut self, ctx: &Context) {
        let wake = ctx.clone();
        match Simulator::start(&self.settings, self.sender.clone(), move || {
            wake.request_repaint();
        }) {
            Ok(simulator) => {
                self.simulator = Some(simulator);
                self.status = format!("listening on {}", self.settings.address());
            }
            Err(e) => self.status = e,
        }
    }

    fn stop_simulator(&mut self) {
        if let Some(simulator) = self.simulator.take() {
            simulator.shutdown();
            self.status = "simulator stopped".into();
        }
    }

    fn connection_bar(&mut self, ui: &mut Ui, ctx: &Context) {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(self.idle(), |ui| {
                let before = self.mode;
                egui::ComboBox::from_id_salt("mode")
                    .width(100.0)
                    .selected_text(match self.mode {
                        Mode::Client => "Client",
                        Mode::Simulator => "Simulator",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.mode, Mode::Client, "Client");
                        ui.selectable_value(&mut self.mode, Mode::Simulator, "Simulator");
                    });
                if self.mode != before {
                    self.view = match self.mode {
                        Mode::Client => View::Telegrams,
                        Mode::Simulator => View::Station,
                    };
                }
            });
            if self.mode == Mode::Simulator {
                self.simulator_bar(ui, ctx);
                return;
            }
            let connected = self.driver.is_some();
            ui.add_enabled_ui(!connected, |ui| {
                ui.label("Host");
                ui.add(egui::TextEdit::singleline(&mut self.settings.host).desired_width(140.0));
                ui.label("Port");
                ui.add(egui::DragValue::new(&mut self.settings.port));
                ui.label("CA");
            });
            ui.add(egui::DragValue::new(&mut self.settings.common_address));
            if connected {
                if ui.button("Disconnect").clicked() {
                    self.disconnect();
                }
            } else if ui.button("Connect").clicked() {
                self.connect(ctx);
            }
            if let Some(driver) = &self.driver {
                if ui.button("STARTDT").clicked() {
                    driver.start();
                }
                if ui.button("STOPDT").clicked() {
                    driver.stop();
                }
            }
            if ui.button("Settings…").clicked() {
                self.show_settings = !self.show_settings;
            }
            ui.separator();
            let state = self.state.map_or("—", state_name);
            ui.label(format!("State: {state}"));
            if let Some(room) = self.window {
                ui.label(format!("k room: {room}/{}", self.settings.k));
            }
        });
    }

    fn simulator_bar(&mut self, ui: &mut Ui, ctx: &Context) {
        let running = self.simulator.is_some();
        ui.add_enabled_ui(!running, |ui| {
            ui.label("Listen");
            ui.add(egui::TextEdit::singleline(&mut self.settings.host).desired_width(140.0));
            ui.label("Port");
            ui.add(egui::DragValue::new(&mut self.settings.port));
            ui.label("CA");
            ui.add(egui::DragValue::new(&mut self.settings.common_address));
            ui.checkbox(&mut self.settings.echo, "Echo commands");
            ui.checkbox(&mut self.settings.select_before_execute, "Require select");
        });
        if running {
            if ui.button("Stop").clicked() {
                self.stop_simulator();
            }
        } else if ui.button("Start").clicked() {
            self.start_simulator(ctx);
        }
        if ui.button("Settings…").clicked() {
            self.show_settings = !self.show_settings;
        }
    }

    /// Returns the rows whose Report and remove buttons were pressed.
    fn station_table(&mut self, ui: &mut Ui) -> (Option<usize>, Option<usize>) {
        let running = self.simulator.is_some();
        let mut report = None;
        let mut remove = None;
        let error = ui.visuals().error_fg_color;
        let height = ui.available_height() - 180.0;
        TableBuilder::new(ui)
            .striped(true)
            .columns(Column::auto(), 7)
            .column(Column::remainder())
            .max_scroll_height(height)
            .header(20.0, |mut header| {
                for title in [
                    "IOA", "Type", "Value", "Quality", "Group", "Time tag", "Commands", "",
                ] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|mut body| {
                for (i, p) in self.settings.station.iter_mut().enumerate() {
                    body.row(24.0, |mut row| {
                        row.col(|ui| {
                            ui.add_enabled(
                                !running,
                                egui::DragValue::new(&mut p.ioa).range(1..=16_777_215),
                            );
                        });
                        row.col(|ui| {
                            ui.add_enabled_ui(!running, |ui| {
                                commands::choice(ui, &format!("kind{i}"), &mut p.kind, &Kind::ALL);
                            });
                        });
                        row.col(|ui| {
                            let invalid = p.parse().is_err();
                            let mut edit =
                                egui::TextEdit::singleline(&mut p.value).desired_width(110.0);
                            if invalid {
                                edit = edit.text_color(error);
                            }
                            let response = ui.add(edit);
                            if running
                                && response.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                            {
                                report = Some(i);
                            }
                        });
                        row.col(|ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            ui.spacing_mut().icon_spacing = 2.0;
                            let element = p.kind != Kind::Counter;
                            let overflows = p.kind.overflows();
                            for (on, label, possible) in [
                                (&mut p.invalid, "IV", true),
                                (&mut p.not_topical, "NT", element),
                                (&mut p.substituted, "SB", element),
                                (&mut p.blocked, "BL", element),
                                (&mut p.overflow, "OV", overflows),
                            ] {
                                ui.add_enabled(possible, egui::Checkbox::new(on, label));
                            }
                        });
                        row.col(|ui| {
                            ui.add_enabled(
                                !running,
                                egui::DragValue::new(&mut p.group).range(0..=16),
                            );
                        });
                        row.col(|ui| {
                            ui.add_enabled(!running, egui::Checkbox::without_text(&mut p.tagged));
                        });
                        row.col(|ui| {
                            let possible = !running && p.kind.family().is_some();
                            ui.add_enabled(possible, egui::Checkbox::without_text(&mut p.commands));
                        });
                        row.col(|ui| {
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(running, egui::Button::new("Report"))
                                    .clicked()
                                {
                                    report = Some(i);
                                }
                                if ui
                                    .add_enabled(!running, egui::Button::new("Remove"))
                                    .clicked()
                                {
                                    remove = Some(i);
                                }
                            });
                        });
                    });
                }
            });
        (report, remove)
    }

    fn station_view(&mut self, ui: &mut Ui) {
        let running = self.simulator.is_some();
        ui.horizontal(|ui| {
            if ui
                .add_enabled(!running, egui::Button::new("Add point"))
                .clicked()
            {
                let next = self
                    .settings
                    .station
                    .iter()
                    .map(|p| p.ioa)
                    .max()
                    .unwrap_or(0);
                self.settings
                    .station
                    .push(SimPoint::new(next.saturating_add(1)));
            }
            ui.label(if running {
                "Edit a value and press Report (or Enter) to send it as a spontaneous event."
            } else {
                "Points are fixed while the station runs."
            });
        });
        let (report, remove) = self.station_table(ui);
        if let (Some(i), Some(simulator)) = (report, &self.simulator)
            && let Some(point) = self.settings.station.get(i)
            && let Err(e) = simulator.report(point)
        {
            self.status = e;
        }
        if let Some(i) = remove {
            self.settings.station.remove(i);
        }
        ui.separator();
        self.served_files(ui);
        ui.separator();
        egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for line in &self.log {
                    ui.monospace(line);
                }
            });
    }

    fn send_ctx(&self) -> commands::Context {
        commands::Context {
            sizes: self.sizes(),
            common_address: self.settings.common_address,
            originator: self.settings.originator,
        }
    }

    fn start_file(&mut self, started: Result<Vec<u8>, String>) {
        match (started, &self.driver) {
            (Ok(asdu), Some(driver)) => driver.send(asdu),
            (Ok(_), None) => {
                self.files.cancel();
                self.files.status = "not connected".into();
            }
            (Err(e), _) => self.files.status = e,
        }
    }

    fn download(&mut self, ioa: u32, nof: u16) {
        let Some(path) = rfd::FileDialog::new()
            .set_file_name(format!("file-{ioa}-{nof}.bin"))
            .save_file()
        else {
            return;
        };
        let ctx = self.send_ctx();
        let started = self.files.download(&ctx, ioa, nof, path);
        self.start_file(started);
    }

    fn upload(&mut self, ioa: u32, nof: u16) {
        let Some(path) = rfd::FileDialog::new().pick_file() else {
            return;
        };
        let started = match std::fs::read(&path) {
            Ok(content) => {
                let ctx = self.send_ctx();
                self.files.upload(&ctx, ioa, nof, content)
            }
            Err(e) => Err(format!("{}: {e}", path.display())),
        };
        self.start_file(started);
    }

    fn served_files(&mut self, ui: &mut Ui) {
        let running = self.simulator.is_some();
        ui.horizontal(|ui| {
            ui.strong("Served files");
            if ui
                .add_enabled(!running, egui::Button::new("Add file…"))
                .clicked()
                && let Some(path) = rfd::FileDialog::new().pick_file()
            {
                let ioa = self.settings.served.iter().map(|f| f.ioa).max();
                let nof = self.settings.served.iter().map(|f| f.nof).max();
                self.settings.served.push(ServedFile {
                    ioa: ioa.map_or(30000, |n| n.saturating_add(1)),
                    nof: nof.map_or(1, |n| n.saturating_add(1)),
                    path: path.display().to_string(),
                });
            }
        });
        let mut remove = None;
        egui::Grid::new("served").num_columns(4).show(ui, |ui| {
            for (i, file) in self.settings.served.iter_mut().enumerate() {
                ui.add_enabled(
                    !running,
                    egui::DragValue::new(&mut file.ioa)
                        .prefix("IOA ")
                        .range(1..=16_777_215),
                );
                ui.add_enabled(
                    !running,
                    egui::DragValue::new(&mut file.nof).prefix("name "),
                );
                ui.label(&file.path);
                if ui
                    .add_enabled(!running, egui::Button::new("Remove"))
                    .clicked()
                {
                    remove = Some(i);
                }
                ui.end_row();
            }
        });
        if let Some(i) = remove {
            self.settings.served.remove(i);
        }
    }

    fn files_view(&mut self, ui: &mut Ui) {
        let busy = self.files.busy();
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal(|ui| {
                ui.label("Directory at IOA");
                ui.add(
                    egui::TextEdit::singleline(&mut self.files.directory_ioa).desired_width(80.0),
                );
                if ui.button("Call directory").clicked() {
                    let ctx = self.send_ctx();
                    let started = self.files.call_directory(&ctx);
                    self.start_file(started);
                }
            });
            ui.horizontal(|ui| {
                ui.label("File at IOA");
                ui.add(egui::TextEdit::singleline(&mut self.files.file_ioa).desired_width(80.0));
                ui.label("name (NOF)");
                ui.add(egui::TextEdit::singleline(&mut self.files.file_nof).desired_width(60.0));
                let address = (
                    self.files.file_ioa.trim().parse(),
                    self.files.file_nof.trim().parse(),
                );
                let download = ui.button("Download…").clicked();
                let upload = ui.button("Upload…").clicked();
                if download || upload {
                    match address {
                        (Ok(ioa), Ok(nof)) if download => self.download(ioa, nof),
                        (Ok(ioa), Ok(nof)) => self.upload(ioa, nof),
                        _ => self.files.status = "IOA and name must be numbers".into(),
                    }
                }
            });
        });
        ui.horizontal(|ui| {
            if let Some(progress) = self.files.progress() {
                ui.spinner();
                ui.label(progress);
                if ui.button("Cancel").clicked() {
                    self.files.cancel();
                }
            } else {
                ui.label(&self.files.status);
            }
        });
        ui.separator();
        let mut chosen = None;
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 8.0;
        TableBuilder::new(ui)
            .striped(true)
            .columns(Column::auto().at_least(60.0), 5)
            .column(Column::remainder())
            .header(20.0, |mut header| {
                for title in ["IOA", "Name", "Length", "Kind", "Time", ""] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, self.files.listing.len(), |mut row| {
                    let Some(file) = self.files.listing.get(row.index()) else {
                        return;
                    };
                    let sof = file.entry.sof;
                    let kind = match (sof.name_defines_subdirectory, sof.transfer_active) {
                        (true, _) => "directory",
                        (false, true) => "file (transfer active)",
                        (false, false) => "file",
                    };
                    for text in [
                        file.ioa.to_string(),
                        file.entry.nof.value.to_string(),
                        file.entry.lof.value.to_string(),
                        kind.to_string(),
                        entry_time(file),
                    ] {
                        row.col(|ui| {
                            ui.monospace(text);
                        });
                    }
                    row.col(|ui| {
                        let enabled = !busy && !sof.name_defines_subdirectory;
                        if ui
                            .add_enabled(enabled, egui::Button::new("Download…"))
                            .clicked()
                        {
                            chosen = Some((file.ioa, file.entry.nof.value));
                        }
                    });
                });
            });
        if let Some((ioa, nof)) = chosen {
            self.download(ioa, nof);
        }
    }

    fn menu(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui
                    .add_enabled(self.idle(), egui::Button::new("Open profile…"))
                    .clicked()
                {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Profile", &["toml"])
                        .pick_file()
                    {
                        match Settings::load(&path) {
                            Ok(s) => self.settings = s,
                            Err(e) => self.status = e,
                        }
                    }
                    ui.close();
                }
                if ui.button("Save profile…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Profile", &["toml"])
                        .save_file()
                    {
                        self.status = self
                            .settings
                            .save(&path)
                            .err()
                            .unwrap_or_else(|| "profile saved".into());
                    }
                    ui.close();
                }
            });
            ui.menu_button("Telegrams", |ui| {
                if ui.button("Save recording…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Log", &["log"])
                        .save_file()
                    {
                        self.status = crate::telegram::save(&path, &self.telegrams)
                            .err()
                            .unwrap_or_else(|| format!("{} telegrams saved", self.telegrams.len()));
                    }
                    ui.close();
                }
                if ui.button("Open recording…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("Log", &["log"])
                        .pick_file()
                    {
                        self.load_recording(&path);
                    }
                    ui.close();
                }
                if ui.button("Clear").clicked() {
                    self.telegrams.clear();
                    self.shown.clear();
                    self.open.clear();
                    self.selected = None;
                    ui.close();
                }
            });
            ui.menu_button("Points", |ui| {
                if ui.button("Export CSV…").clicked() {
                    if let Some(path) = rfd::FileDialog::new()
                        .add_filter("CSV", &["csv"])
                        .save_file()
                    {
                        self.status = self
                            .points
                            .save_csv(&path)
                            .err()
                            .unwrap_or_else(|| "points exported".into());
                    }
                    ui.close();
                }
                if ui.button("Clear").clicked() {
                    self.points = Points::default();
                    ui.close();
                }
            });
        });
    }

    fn load_recording(&mut self, path: &std::path::Path) {
        let sizes = self.sizes();
        match crate::telegram::load(path, sizes) {
            Ok(loaded) => {
                self.telegrams = loaded;
                self.points = Points::default();
                for t in &self.telegrams {
                    if let Some(asdu) = t.asdu().filter(|_| t.dir == Dir::Rx) {
                        self.points.apply(asdu, t.sizes, t.at);
                    }
                }
                self.open.clear();
                self.selected = None;
                self.refilter();
                self.status = format!("{} telegrams loaded", self.telegrams.len());
            }
            Err(e) => self.status = e,
        }
    }

    fn settings_window(&mut self, ctx: &Context) {
        let editable = self.idle();
        let s = &mut self.settings;
        egui::Window::new("Settings")
            .open(&mut self.show_settings)
            .resizable(false)
            .show(ctx, |ui| {
                ui.add_enabled_ui(editable, |ui| {
                    egui::Grid::new("settings").num_columns(2).show(ui, |ui| {
                        ui.label("Originator address");
                        ui.add(egui::DragValue::new(&mut s.originator));
                        ui.end_row();
                        ui.label("COT octets");
                        ui.add(egui::DragValue::new(&mut s.cot_octets).range(1..=2));
                        ui.end_row();
                        ui.label("CA octets");
                        ui.add(egui::DragValue::new(&mut s.ca_octets).range(1..=2));
                        ui.end_row();
                        ui.label("IOA octets");
                        ui.add(egui::DragValue::new(&mut s.ioa_octets).range(1..=3));
                        ui.end_row();
                        ui.label("k");
                        ui.add(egui::DragValue::new(&mut s.k).range(1..=32767));
                        ui.end_row();
                        ui.label("w");
                        ui.add(egui::DragValue::new(&mut s.w).range(1..=32767));
                        ui.end_row();
                        for (label, value) in [
                            ("t0 (s)", &mut s.t0),
                            ("t1 (s)", &mut s.t1),
                            ("t2 (s)", &mut s.t2),
                            ("t3 (s)", &mut s.t3),
                        ] {
                            ui.label(label);
                            ui.add(egui::DragValue::new(value).range(1..=255));
                            ui.end_row();
                        }
                        ui.label("STARTDT on connect");
                        ui.checkbox(&mut s.auto_start, "");
                        ui.end_row();
                        ui.label("TLS");
                        ui.checkbox(&mut s.tls.enabled, "mutual TLS");
                        ui.end_row();
                    });
                    if s.tls.enabled {
                        for (label, path) in [
                            ("Certificate (PEM)", &mut s.tls.certificate),
                            ("Private key (PEM)", &mut s.tls.key),
                            ("CA (PEM)", &mut s.tls.ca),
                            ("CRL (PEM, optional)", &mut s.tls.crl),
                            ("Allowed peers (PEM, optional)", &mut s.tls.allowed_peers),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(label);
                                ui.text_edit_singleline(path);
                                if ui.button("…").clicked()
                                    && let Some(p) = rfd::FileDialog::new().pick_file()
                                {
                                    *path = p.display().to_string();
                                }
                            });
                        }
                        ui.checkbox(&mut s.tls.verify_server_name, "Verify server name");
                        for (label, version) in [
                            ("Minimum version", &mut s.tls.min_version),
                            ("Maximum version", &mut s.tls.max_version),
                        ] {
                            ui.horizontal(|ui| {
                                ui.label(label);
                                ui.selectable_value(version, TlsVersion::Tls12, "TLS 1.2");
                                ui.selectable_value(version, TlsVersion::Tls13, "TLS 1.3");
                            });
                        }
                        ui.add_enabled_ui(!s.tls.crl.is_empty(), |ui| {
                            ui.checkbox(
                                &mut s.tls.crl_end_entity_only,
                                "Check revocation of the peer's certificate only",
                            );
                            ui.checkbox(
                                &mut s.tls.allow_unknown_revocation_status,
                                "Accept certificates no CRL covers",
                            );
                        });
                        ui.horizontal(|ui| {
                            ui.label("Handshake timeout (s)");
                            ui.add(
                                egui::DragValue::new(&mut s.tls.handshake_timeout).range(1..=255),
                            );
                        });
                    }
                    if let Err(e) = s.link().and_then(|_| s.sizes()) {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    if ui.button("Reset to defaults").clicked() {
                        *s = Settings::default();
                    }
                });
                if !editable {
                    ui.label("Disconnect or stop the simulator to change settings.");
                }
            });
    }

    fn filter_bar(&mut self, ui: &mut Ui) {
        let before = self.filter.clone();
        ui.horizontal_wrapped(|ui| {
            for (label, text) in [
                ("CA", &mut self.filter.ca),
                ("IOA", &mut self.filter.ioa),
                ("Types", &mut self.filter.types),
                ("Causes", &mut self.filter.causes),
            ] {
                ui.label(label);
                ui.add(egui::TextEdit::singleline(text).desired_width(70.0));
            }
            ui.checkbox(&mut self.filter.tx, "TX");
            ui.checkbox(&mut self.filter.rx, "RX");
            ui.menu_button("Categories", |ui| {
                for (on, category) in self.filter.categories.iter_mut().zip(Category::ALL) {
                    ui.checkbox(on, category.label());
                }
            });
            if ui.button("Reset").clicked() {
                self.filter = Filter::default();
            }
            ui.checkbox(&mut self.follow, "Follow");
            ui.label(format!("{} / {}", self.shown.len(), self.telegrams.len()));
        });
        if self.filter != before {
            self.refilter();
        }
    }

    fn telegram_table(&mut self, ui: &mut Ui) {
        let mut clicked = None;
        let mut opened = None;
        // Selectable labels would take the clicks meant for the row.
        ui.style_mut().interaction.selectable_labels = false;
        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
        let mut table = TableBuilder::new(ui)
            .striped(true)
            .sense(egui::Sense::click())
            .column(Column::auto().at_least(100.0))
            .column(Column::auto().at_least(28.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(60.0))
            .column(Column::auto().at_least(40.0))
            .column(Column::remainder())
            .min_scrolled_height(0.0);
        if self.follow {
            table = table.stick_to_bottom(true);
        }
        table
            .header(20.0, |mut header| {
                for title in ["Time", "Dir", "Type", "Cause", "CA", "Summary"] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, self.shown.len(), |mut row| {
                    let Some(&index) = self.shown.get(row.index()) else {
                        return;
                    };
                    let Some(t) = self.telegrams.get(index) else {
                        return;
                    };
                    row.set_selected(self.selected == Some(index));
                    row.col(|ui| {
                        ui.monospace(t.at.format("%T%.3f").to_string());
                    });
                    row.col(|ui| {
                        let color = match t.dir {
                            Dir::Tx => egui::Color32::from_rgb(70, 130, 220),
                            Dir::Rx => egui::Color32::from_rgb(60, 170, 90),
                        };
                        ui.label(RichText::new(t.dir.label()).color(color).monospace());
                    });
                    row.col(|ui| {
                        ui.monospace(t.header.map_or("", |h| h.type_id.name()));
                    });
                    row.col(|ui| {
                        let cause = t.header.map(|h| {
                            format!(
                                "{}{}",
                                cause_name(h.cause),
                                if h.negative { " neg" } else { "" }
                            )
                        });
                        ui.monospace(cause.unwrap_or_default());
                    });
                    row.col(|ui| {
                        ui.monospace(
                            t.header
                                .map(|h| h.common_address.to_string())
                                .unwrap_or_default(),
                        );
                    });
                    row.col(|ui| {
                        ui.add(egui::Label::new(RichText::new(&t.summary).monospace()).truncate());
                    });
                    let response = row.response();
                    if response.clicked() {
                        clicked = Some(index);
                    }
                    if response.double_clicked() {
                        opened = Some(index);
                    }
                    response.context_menu(|ui| {
                        if row_menu(ui, t) {
                            opened = Some(index);
                        }
                    });
                });
            });
        if let Some(index) = clicked {
            self.selected = Some(index);
            self.follow = false;
        }
        if let Some(index) = opened
            && !self.open.contains(&index)
        {
            self.open.push(index);
        }
    }

    fn points_table(&self, ui: &mut Ui) {
        let row_height = ui.text_style_height(&egui::TextStyle::Monospace) + 4.0;
        let rows: Vec<_> = self.points.rows.iter().collect();
        TableBuilder::new(ui)
            .striped(true)
            .column(Column::auto().at_least(40.0))
            .column(Column::auto().at_least(60.0))
            .column(Column::auto().at_least(90.0))
            .column(Column::auto().at_least(120.0))
            .column(Column::auto().at_least(60.0))
            .column(Column::auto().at_least(180.0))
            .column(Column::auto().at_least(60.0))
            .column(Column::auto().at_least(50.0))
            .column(Column::remainder())
            .header(20.0, |mut header| {
                for title in [
                    "CA", "IOA", "Type", "Value", "Quality", "Time tag", "Cause", "Count",
                    "Received",
                ] {
                    header.col(|ui| {
                        ui.strong(title);
                    });
                }
            })
            .body(|body| {
                body.rows(row_height, rows.len(), |mut row| {
                    let Some(((ca, ioa), r)) = rows.get(row.index()) else {
                        return;
                    };
                    for text in [
                        ca.to_string(),
                        ioa.to_string(),
                        r.type_name.to_string(),
                        r.value.clone(),
                        r.quality.clone(),
                        r.time.clone(),
                        r.cause.clone(),
                        r.updates.to_string(),
                        r.received.format("%T%.3f").to_string(),
                    ] {
                        row.col(|ui| {
                            ui.monospace(text);
                        });
                    }
                });
            });
    }

    fn detail_windows(&mut self, ctx: &Context) {
        let telegrams = &self.telegrams;
        self.open.retain(|&index| {
            let Some(t) = telegrams.get(index) else {
                return false;
            };
            let mut open = true;
            egui::Window::new(format!("Telegram #{index}"))
                .open(&mut open)
                .default_width(520.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label(RichText::new(t.details()).monospace());
                    });
                });
            open
        });
    }
}

impl eframe::App for App {
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, SETTINGS_KEY, &self.settings);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.disconnect();
        self.stop_simulator();
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain();
        if let Some(pending) = &self.pending {
            if pending.sent.elapsed() >= pending.timeout {
                self.status = format!(
                    "IOA {}: no select confirmation within t1, not executed",
                    pending.ioa
                );
                self.pending = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(250));
            }
        }
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            self.menu(ui);
            self.connection_bar(ui, ctx);
        });
        egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.label(&self.status);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(Local::now().format("%T").to_string());
                });
            });
        });
        egui::SidePanel::right("send")
            .resizable(true)
            .default_width(320.0)
            .show_animated(ctx, self.mode == Mode::Client, |ui| {
                ui.heading("Send");
                let send_ctx = self.send_ctx();
                egui::ScrollArea::vertical().show(ui, |ui| match self.forms.ui(ui, &send_ctx) {
                    Some(Ok(action)) => self.act(action),
                    Some(Err(e)) => self.status = e,
                    None => {}
                });
            });
        if self.view == View::Telegrams
            && let Some(t) = self.selected.and_then(|i| self.telegrams.get(i))
        {
            let details = t.details();
            egui::TopBottomPanel::bottom("detail")
                .resizable(true)
                .default_height(220.0)
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        ui.label(RichText::new(details).monospace());
                    });
                });
        }
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.view, View::Telegrams, "Telegrams");
                match self.mode {
                    Mode::Client => {
                        ui.selectable_value(
                            &mut self.view,
                            View::Points,
                            format!("Points ({})", self.points.rows.len()),
                        );
                        ui.selectable_value(&mut self.view, View::Files, "Files")
                    }
                    Mode::Simulator => ui.selectable_value(
                        &mut self.view,
                        View::Station,
                        format!("Station ({})", self.settings.station.len()),
                    ),
                };
            });
            ui.separator();
            match self.view {
                View::Telegrams => {
                    self.filter_bar(ui);
                    self.telegram_table(ui);
                }
                View::Points => self.points_table(ui),
                View::Files => self.files_view(ui),
                View::Station => self.station_view(ui),
            }
        });
        self.settings_window(ctx);
        self.detail_windows(ctx);
    }
}

fn row_menu(ui: &mut Ui, t: &Telegram) -> bool {
    if ui.button("Copy hex").clicked() {
        ui.ctx().copy_text(hex(&t.bytes));
        ui.close();
    }
    if ui.button("Copy as C array").clicked() {
        let items: Vec<String> = t.bytes.iter().map(|b| format!("0x{b:02X}")).collect();
        ui.ctx().copy_text(format!("{{ {} }}", items.join(", ")));
        ui.close();
    }
    if ui.button("Copy summary").clicked() {
        ui.ctx().copy_text(t.summary.clone());
        ui.close();
    }
    let open = ui.button("Open details").clicked();
    if open {
        ui.close();
    }
    open
}
