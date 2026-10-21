//! The main window.

use std::sync::mpsc;

use chrono::Local;
use egui::{Context, RichText, Ui};
use egui_extras::{Column, TableBuilder};
use iec104::link::State;
use iec104_tokio::Event;

use crate::commands::{self, Forms};
use crate::driver::{Driver, Notice, NoticeKind, state_name};
use crate::points::Points;
use crate::settings::Settings;
use crate::telegram::{Category, Dir, Filter, Matcher, Telegram, cause_name, hex};

const SETTINGS_KEY: &str = "settings";

#[derive(Clone, Copy, PartialEq, Eq)]
enum View {
    Telegrams,
    Points,
}

pub struct App {
    settings: Settings,
    driver: Option<Driver>,
    notices: mpsc::Receiver<Notice>,
    sender: mpsc::Sender<Notice>,
    state: Option<State>,
    window: Option<u16>,
    telegrams: Vec<Telegram>,
    /// Indices into `telegrams` that pass the filter.
    shown: Vec<usize>,
    filter: Filter,
    matcher: Matcher,
    follow: bool,
    selected: Option<usize>,
    /// Detail windows, by telegram index.
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
            driver: None,
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

    fn sizes(&self) -> iec104::asdu::AddressSizes {
        self.settings
            .sizes()
            .unwrap_or(iec104::asdu::AddressSizes::CONVENTIONAL)
    }

    fn push(&mut self, telegram: Telegram) {
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
                    Event::Asdu { asdu, .. } => self.points.apply(&asdu, sizes, notice.at),
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
                    }
                    _ => {}
                },
                NoticeKind::Window(room) => self.window = Some(room),
                NoticeKind::Error(e) => self.status = e,
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

    fn connection_bar(&mut self, ui: &mut Ui, ctx: &Context) {
        ui.horizontal(|ui| {
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

    fn menu(&mut self, ui: &mut Ui) {
        egui::MenuBar::new().ui(ui, |ui| {
            ui.menu_button("File", |ui| {
                if ui.button("Open profile…").clicked() {
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
                        self.points.apply(asdu, sizes, t.at);
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
        let editable = self.driver.is_none();
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
                    }
                    if let Err(e) = s.link().and_then(|_| s.sizes()) {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    if ui.button("Reset to defaults").clicked() {
                        *s = Settings::default();
                    }
                });
                if !editable {
                    ui.label("Disconnect to change settings.");
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
                        ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
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
        let sizes = self.sizes();
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
                        ui.label(RichText::new(t.details(sizes)).monospace());
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
    }

    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        self.drain();
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
            .show(ctx, |ui| {
                ui.heading("Send");
                let send_ctx = commands::Context {
                    sizes: self.sizes(),
                    common_address: self.settings.common_address,
                    originator: self.settings.originator,
                };
                egui::ScrollArea::vertical().show(ui, |ui| match self.forms.ui(ui, &send_ctx) {
                    Some(Ok(asdu)) => match &self.driver {
                        Some(driver) => driver.send(asdu),
                        None => self.status = "not connected".into(),
                    },
                    Some(Err(e)) => self.status = e,
                    None => {}
                });
            });
        if self.view == View::Telegrams
            && let Some(t) = self.selected.and_then(|i| self.telegrams.get(i))
        {
            let details = t.details(self.sizes());
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
                ui.selectable_value(
                    &mut self.view,
                    View::Points,
                    format!("Points ({})", self.points.rows.len()),
                );
            });
            ui.separator();
            match self.view {
                View::Telegrams => {
                    self.filter_bar(ui);
                    self.telegram_table(ui);
                }
                View::Points => self.points_table(ui),
            }
        });
        self.settings_window(ctx);
        self.detail_windows(ctx);
    }
}

/// The telegram row's context menu. Returns whether "Open details" was chosen.
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
