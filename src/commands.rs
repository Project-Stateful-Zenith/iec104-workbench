use std::str::FromStr;
use std::time::SystemTime;

use egui::Ui;
use iec104::asdu::layout::{
    CBoNa1, CBoTa1, CCiNa1, CCsNa1, CDcNa1, CDcTa1, CIcNa1, CRcNa1, CRcTa1, CRdNa1, CRpNa1, CScNa1,
    CScTa1, CSeNa1, CSeNb1, CSeNc1, CSeTa1, CSeTb1, CSeTc1, CTsTa1, LayoutObject, MBoNa1, MBoTb1,
    MDpNa1, MDpTb1, MItNa1, MItTb1, MMeNa1, MMeNb1, MMeNc1, MMeTd1, MMeTe1, MMeTf1, MSpNa1, MSpTb1,
    MStNa1, MStTb1, PAcNa1, PMeNa1, PMeNb1, PMeNc1,
};
use iec104::asdu::{
    AddressSizes, Asdu, Bcr, Bsi, Cause, Cp56Time2a, Dco, Diq, DoubleCommandState,
    DoublePointState, FreezeMode, Header, Nva, Objects, Qcc, Qds, Qoc, Qoi, Qos, Qpa, Qpm, Qrp,
    R32, Rco, RegulatingStepState, Sco, Siq, Sva, Tsc, Vti,
};

pub enum Action {
    Send(Vec<u8>),
    /// Send `select`, then `execute` once the station confirms the select.
    SelectExecute {
        select: Vec<u8>,
        execute: Vec<u8>,
        type_id: u8,
        ioa: u32,
    },
}

pub struct Context {
    pub sizes: AddressSizes,
    pub common_address: u16,
    pub originator: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Process,
    System,
    Parameter,
    Monitor,
    Manual,
}

#[expect(clippy::struct_excessive_bools, reason = "one field per checkbox")]
pub struct Forms {
    pub tab: Tab,
    pub cause: u8,
    pub negative: bool,
    pub test: bool,
    pub ioa: String,
    pub time_tag: bool,
    pub process: ProcessKind,
    /// Single: ON; double: index into ON/OFF; regulating: higher.
    pub on: bool,
    pub select: bool,
    /// QU of a command, QL of a set point, QOI, QRP, QPA or KPA.
    pub qualifier: String,
    /// Set point, parameter or monitored value, or the bitstring in hex.
    pub value: String,
    pub system: SystemKind,
    pub freeze: FreezeMode,
    pub test_counter: u16,
    pub parameter: ParameterKind,
    pub monitor: MonitorKind,
    pub double: DoublePointState,
    pub quality: Qds,
    pub manual: String,
}

impl Default for Forms {
    fn default() -> Self {
        Self {
            tab: Tab::Process,
            cause: 6,
            negative: false,
            test: false,
            ioa: "1".into(),
            time_tag: false,
            process: ProcessKind::Single,
            on: true,
            select: false,
            qualifier: "0".into(),
            value: "0".into(),
            system: SystemKind::Interrogation,
            freeze: FreezeMode::Read,
            test_counter: 0,
            parameter: ParameterKind::Scaled,
            monitor: MonitorKind::Single,
            double: DoublePointState::On,
            quality: Qds::default(),
            manual: String::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProcessKind {
    Single,
    Double,
    Regulating,
    Normalized,
    Scaled,
    Float,
    Bitstring,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemKind {
    Interrogation,
    CounterInterrogation,
    Read,
    ClockSync,
    Test,
    ResetProcess,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParameterKind {
    Normalized,
    Scaled,
    Float,
    Activation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorKind {
    Single,
    Double,
    Step,
    Bitstring,
    Normalized,
    Scaled,
    Float,
    Counter,
}

const PROCESS: [(ProcessKind, &str); 7] = [
    (ProcessKind::Single, "Single command (C_SC)"),
    (ProcessKind::Double, "Double command (C_DC)"),
    (ProcessKind::Regulating, "Regulating step (C_RC)"),
    (ProcessKind::Normalized, "Set point normalized (C_SE_A)"),
    (ProcessKind::Scaled, "Set point scaled (C_SE_B)"),
    (ProcessKind::Float, "Set point float (C_SE_C)"),
    (ProcessKind::Bitstring, "Bitstring 32 (C_BO)"),
];

const SYSTEM: [(SystemKind, &str); 6] = [
    (SystemKind::Interrogation, "Interrogation (C_IC_NA_1)"),
    (
        SystemKind::CounterInterrogation,
        "Counter interrogation (C_CI_NA_1)",
    ),
    (SystemKind::Read, "Read (C_RD_NA_1)"),
    (SystemKind::ClockSync, "Clock sync (C_CS_NA_1)"),
    (SystemKind::Test, "Test with time (C_TS_TA_1)"),
    (SystemKind::ResetProcess, "Reset process (C_RP_NA_1)"),
];

const PARAMETER: [(ParameterKind, &str); 4] = [
    (ParameterKind::Normalized, "Normalized (P_ME_NA_1)"),
    (ParameterKind::Scaled, "Scaled (P_ME_NB_1)"),
    (ParameterKind::Float, "Float (P_ME_NC_1)"),
    (ParameterKind::Activation, "Activation (P_AC_NA_1)"),
];

const MONITOR: [(MonitorKind, &str); 8] = [
    (MonitorKind::Single, "Single point (M_SP)"),
    (MonitorKind::Double, "Double point (M_DP)"),
    (MonitorKind::Step, "Step position (M_ST)"),
    (MonitorKind::Bitstring, "Bitstring (M_BO)"),
    (MonitorKind::Normalized, "Normalized (M_ME_A/D)"),
    (MonitorKind::Scaled, "Scaled (M_ME_B/E)"),
    (MonitorKind::Float, "Float (M_ME_C/F)"),
    (MonitorKind::Counter, "Integrated total (M_IT)"),
];

const QU: [(&str, &str); 4] = [
    ("0", "0 no additional definition"),
    ("1", "1 short pulse"),
    ("2", "2 long pulse"),
    ("3", "3 persistent output"),
];

fn qu_ui(ui: &mut Ui, text: &mut String) {
    let named = QU
        .iter()
        .find(|(v, _)| *v == text.trim())
        .map(|(_, label)| *label);
    ui.horizontal(|ui| {
        ui.label("QU");
        egui::ComboBox::from_id_salt("qu")
            .selected_text(named.unwrap_or("custom"))
            .width(200.0)
            .show_ui(ui, |ui| {
                for (value, label) in QU {
                    if ui.selectable_label(named == Some(label), label).clicked() {
                        *text = value.into();
                    }
                }
                if ui
                    .selectable_label(named.is_none(), "custom (4–31)")
                    .clicked()
                    && named.is_some()
                {
                    *text = "16".into();
                }
            });
        if named.is_none() {
            ui.add(egui::TextEdit::singleline(text).desired_width(40.0));
        }
    });
}

const FREEZE: [(FreezeMode, &str); 4] = [
    (FreezeMode::Read, "read"),
    (FreezeMode::FreezeWithoutReset, "freeze"),
    (FreezeMode::FreezeWithReset, "freeze + reset"),
    (FreezeMode::Reset, "reset"),
];

const DOUBLE: [(DoublePointState, &str); 4] = [
    (DoublePointState::NotDetermined, "0 indeterminate"),
    (DoublePointState::Off, "1 OFF"),
    (DoublePointState::On, "2 ON"),
    (DoublePointState::Indeterminate, "3 indeterminate"),
];

const CAUSES: [(u8, &str); 9] = [
    (3, "3 spont"),
    (5, "5 req"),
    (6, "6 act"),
    (7, "7 actcon"),
    (8, "8 deact"),
    (9, "9 deactcon"),
    (10, "10 actterm"),
    (11, "11 retrem"),
    (20, "20 inrogen"),
];

pub fn choice<T: Copy + PartialEq>(ui: &mut Ui, id: &str, current: &mut T, options: &[(T, &str)]) {
    let text = options
        .iter()
        .find(|(v, _)| v == current)
        .map_or("", |(_, label)| label);
    egui::ComboBox::from_id_salt(id)
        .selected_text(text)
        .width(220.0)
        .show_ui(ui, |ui| {
            for (value, label) in options {
                ui.selectable_value(current, *value, *label);
            }
        });
}

fn field(ui: &mut Ui, label: &str, text: &mut String) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.add(egui::TextEdit::singleline(text).desired_width(120.0));
    });
}

fn parse<T: FromStr>(text: &str, what: &str) -> Result<T, String> {
    text.trim()
        .parse()
        .map_err(|_| format!("{what}: not a valid number"))
}

macro_rules! tagged {
    ($t:expr, $ioa:expr, $ta:ident, $na:ident { $($field:ident),* }) => {
        if $t {
            Objects::$ta(one($ioa, $ta { $($field,)* time: now()? }))
        } else {
            Objects::$na(one($ioa, $na { $($field),* }))
        }
    };
}

fn one<T>(ioa: u32, value: T) -> Vec<LayoutObject<T>> {
    vec![LayoutObject { ioa, value }]
}

fn now() -> Result<Cp56Time2a, String> {
    Cp56Time2a::from_system_time(SystemTime::now()).map_err(|e| format!("{e:?}"))
}

impl Forms {
    pub fn ui(&mut self, ui: &mut Ui, ctx: &Context) -> Option<Result<Action, String>> {
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
                (Tab::Process, "Process"),
                (Tab::System, "System"),
                (Tab::Parameter, "Parameter"),
                (Tab::Monitor, "Monitor"),
                (Tab::Manual, "Hex"),
            ] {
                if ui.selectable_value(&mut self.tab, tab, label).clicked() {
                    self.cause = self.default_cause();
                    self.qualifier = self.default_qualifier().into();
                }
            }
        });
        ui.separator();
        if self.tab == Tab::Manual {
            return self.manual_ui(ui, ctx).map(|r| r.map(Action::Send));
        }
        self.fields_ui(ui);
        ui.horizontal(|ui| {
            ui.label("Cause");
            choice(ui, "cause", &mut self.cause, &CAUSES);
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.negative, "P/N negative");
            ui.checkbox(&mut self.test, "Test");
        });
        ui.horizontal(|ui| {
            let send = ui.button("Send").clicked();
            let select_execute = self.tab == Tab::Process
                && self.process != ProcessKind::Bitstring
                && ui.button("Select + execute").clicked();
            if select_execute {
                Some(self.select_execute(ctx))
            } else if send {
                Some(self.build(ctx).map(Action::Send))
            } else {
                None
            }
        })
        .inner
    }

    pub fn select_execute(&mut self, ctx: &Context) -> Result<Action, String> {
        let chosen = self.select;
        self.select = true;
        let select = self.build(ctx);
        self.select = false;
        let execute = self.build(ctx);
        self.select = chosen;
        let select = select?;
        Ok(Action::SelectExecute {
            type_id: select.first().copied().unwrap_or_default(),
            ioa: parse(&self.ioa, "IOA")?,
            select,
            execute: execute?,
        })
    }

    /// Read uses cause 5 (request), monitor direction 3, the rest act.
    pub const fn default_cause(&self) -> u8 {
        match (self.tab, self.system) {
            (Tab::Monitor, _) => 3,
            (Tab::System, SystemKind::Read) => 5,
            _ => 6,
        }
    }

    const fn default_qualifier(&self) -> &'static str {
        match (self.tab, self.system) {
            (Tab::System, SystemKind::Interrogation) => "20",
            (Tab::System, SystemKind::CounterInterrogation) => "5",
            (Tab::System, SystemKind::ResetProcess) | (Tab::Parameter, _) => "1",
            _ => "0",
        }
    }

    fn fields_ui(&mut self, ui: &mut Ui) {
        match self.tab {
            Tab::Process => {
                choice(ui, "process", &mut self.process, &PROCESS);
                field(ui, "IOA", &mut self.ioa);
                match self.process {
                    ProcessKind::Single => {
                        ui.checkbox(&mut self.on, "ON");
                    }
                    ProcessKind::Double => {
                        ui.checkbox(&mut self.on, "ON (else OFF)");
                    }
                    ProcessKind::Regulating => {
                        ui.checkbox(&mut self.on, "Higher (else lower)");
                    }
                    ProcessKind::Bitstring => field(ui, "Bits (hex)", &mut self.value),
                    _ => field(ui, "Value", &mut self.value),
                }
                let label = match self.process {
                    ProcessKind::Normalized | ProcessKind::Scaled | ProcessKind::Float => "QL",
                    _ => "QU",
                };
                if label == "QU" && self.process != ProcessKind::Bitstring {
                    qu_ui(ui, &mut self.qualifier);
                } else if self.process != ProcessKind::Bitstring {
                    field(ui, label, &mut self.qualifier);
                }
                if self.process != ProcessKind::Bitstring {
                    ui.checkbox(&mut self.select, "Select (else execute)");
                }
                ui.checkbox(&mut self.time_tag, "With CP56 time tag");
            }
            Tab::System => {
                let before = self.system;
                choice(ui, "system", &mut self.system, &SYSTEM);
                if self.system != before {
                    self.cause = self.default_cause();
                    self.qualifier = self.default_qualifier().into();
                }
                match self.system {
                    SystemKind::Interrogation => {
                        field(ui, "QOI (20 = station)", &mut self.qualifier);
                    }
                    SystemKind::CounterInterrogation => {
                        field(ui, "RQT (5 = general)", &mut self.qualifier);
                        choice(ui, "freeze", &mut self.freeze, &FREEZE);
                    }
                    SystemKind::Read => field(ui, "IOA", &mut self.ioa),
                    SystemKind::ResetProcess => field(ui, "QRP (1 = general)", &mut self.qualifier),
                    SystemKind::ClockSync | SystemKind::Test => {
                        ui.label("Sends the current UTC time.");
                    }
                }
            }
            Tab::Parameter => {
                let before = self.parameter;
                choice(ui, "parameter", &mut self.parameter, &PARAMETER);
                if self.parameter != before {
                    self.qualifier = self.default_qualifier().into();
                }
                field(ui, "IOA", &mut self.ioa);
                if self.parameter == ParameterKind::Activation {
                    field(ui, "QPA", &mut self.qualifier);
                } else {
                    field(ui, "Value", &mut self.value);
                    field(
                        ui,
                        "KPA (1 thr, 2 smooth, 3 low, 4 high)",
                        &mut self.qualifier,
                    );
                }
            }
            Tab::Monitor => {
                choice(ui, "monitor", &mut self.monitor, &MONITOR);
                field(ui, "IOA", &mut self.ioa);
                match self.monitor {
                    MonitorKind::Single => {
                        ui.checkbox(&mut self.on, "ON");
                    }
                    MonitorKind::Double => choice(ui, "double", &mut self.double, &DOUBLE),
                    MonitorKind::Bitstring => field(ui, "Bits (hex)", &mut self.value),
                    _ => field(ui, "Value", &mut self.value),
                }
                ui.horizontal_wrapped(|ui| {
                    ui.checkbox(&mut self.quality.invalid, "IV");
                    ui.checkbox(&mut self.quality.not_topical, "NT");
                    ui.checkbox(&mut self.quality.substituted, "SB");
                    ui.checkbox(&mut self.quality.blocked, "BL");
                    ui.checkbox(&mut self.quality.overflow, "OV");
                });
                ui.checkbox(&mut self.time_tag, "With CP56 time tag");
            }
            Tab::Manual => {}
        }
    }

    fn manual_ui(&mut self, ui: &mut Ui, ctx: &Context) -> Option<Result<Vec<u8>, String>> {
        ui.label("ASDU octets in hex. They go out as one I-frame.");
        ui.add(
            egui::TextEdit::multiline(&mut self.manual)
                .code_editor()
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        let bytes = crate::telegram::parse_hex(&self.manual);
        match &bytes {
            Ok(b) if !b.is_empty() => {
                let text = iec104::asdu::decode_with(b, ctx.sizes)
                    .map_or_else(|e| format!("{e:?}"), |a| format!("{:#?}", a.header));
                ui.label(egui::RichText::new(text).monospace().small());
            }
            Ok(_) => {}
            Err(e) => {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
        }
        ui.button("Send").clicked().then_some(bytes)
    }

    pub fn build(&mut self, ctx: &Context) -> Result<Vec<u8>, String> {
        let ioa: u32 = parse(&self.ioa, "IOA")?;
        let objects = match self.tab {
            Tab::Process => self.process_objects(ioa)?,
            Tab::System => self.system_objects()?,
            Tab::Parameter => self.parameter_objects(ioa)?,
            Tab::Monitor => self.monitor_objects(ioa)?,
            Tab::Manual => return crate::telegram::parse_hex(&self.manual),
        };
        let header = Header {
            type_id: objects.type_id(),
            sequence: false,
            count: 1,
            cause: Cause::from_u8(self.cause),
            negative: self.negative,
            test: self.test,
            originator: ctx.originator,
            common_address: ctx.common_address,
        };
        Asdu { header, objects }
            .encode_with(ctx.sizes)
            .map_err(|e| format!("{e:?}"))
    }

    fn process_objects(&self, ioa: u32) -> Result<Objects, String> {
        let t = self.time_tag;
        let qoc = || -> Result<Qoc, String> {
            Ok(Qoc {
                qu: parse(&self.qualifier, "QU")?,
                select: self.select,
            })
        };
        let qos = || -> Result<Qos, String> {
            Ok(Qos {
                ql: parse(&self.qualifier, "QL")?,
                select: self.select,
            })
        };
        Ok(match self.process {
            ProcessKind::Single => {
                let sco = Sco {
                    scs: self.on,
                    qoc: qoc()?,
                };
                tagged!(t, ioa, CScTa1, CScNa1 { sco })
            }
            ProcessKind::Double => {
                let dcs = if self.on {
                    DoubleCommandState::On
                } else {
                    DoubleCommandState::Off
                };
                let dco = Dco { dcs, qoc: qoc()? };
                tagged!(t, ioa, CDcTa1, CDcNa1 { dco })
            }
            ProcessKind::Regulating => {
                let rcs = if self.on {
                    RegulatingStepState::Higher
                } else {
                    RegulatingStepState::Lower
                };
                let rco = Rco { rcs, qoc: qoc()? };
                tagged!(t, ioa, CRcTa1, CRcNa1 { rco })
            }
            ProcessKind::Normalized => {
                let nva = normalized(&self.value)?;
                let qos = qos()?;
                tagged!(t, ioa, CSeTa1, CSeNa1 { nva, qos })
            }
            ProcessKind::Scaled => {
                let sva = Sva::from_raw(parse(&self.value, "value")?);
                let qos = qos()?;
                tagged!(t, ioa, CSeTb1, CSeNb1 { sva, qos })
            }
            ProcessKind::Float => {
                let r32 = R32::from_raw(parse(&self.value, "value")?);
                let qos = qos()?;
                tagged!(t, ioa, CSeTc1, CSeNc1 { r32, qos })
            }
            ProcessKind::Bitstring => {
                let bsi = Bsi {
                    bits: hex_u32(&self.value)?,
                };
                tagged!(t, ioa, CBoTa1, CBoNa1 { bsi })
            }
        })
    }

    fn system_objects(&mut self) -> Result<Objects, String> {
        Ok(match self.system {
            SystemKind::Interrogation => Objects::CIcNa1(one(
                0,
                CIcNa1 {
                    qoi: Qoi {
                        value: parse(&self.qualifier, "QOI")?,
                    },
                },
            )),
            SystemKind::CounterInterrogation => Objects::CCiNa1(one(
                0,
                CCiNa1 {
                    qcc: Qcc {
                        rqt: parse(&self.qualifier, "RQT")?,
                        frz: self.freeze,
                    },
                },
            )),
            SystemKind::Read => Objects::CRdNa1(one(parse(&self.ioa, "IOA")?, CRdNa1)),
            SystemKind::ClockSync => Objects::CCsNa1(one(0, CCsNa1 { time: now()? })),
            SystemKind::Test => {
                self.test_counter = self.test_counter.wrapping_add(1);
                Objects::CTsTa1(one(
                    0,
                    CTsTa1 {
                        tsc: Tsc::from_raw(self.test_counter),
                        time: now()?,
                    },
                ))
            }
            SystemKind::ResetProcess => Objects::CRpNa1(one(
                0,
                CRpNa1 {
                    qrp: Qrp {
                        value: parse(&self.qualifier, "QRP")?,
                    },
                },
            )),
        })
    }

    fn parameter_objects(&self, ioa: u32) -> Result<Objects, String> {
        let qpm = || -> Result<Qpm, String> {
            Ok(Qpm {
                kpa: parse(&self.qualifier, "KPA")?,
                local_parameter_change: false,
                parameter_not_in_operation: false,
            })
        };
        Ok(match self.parameter {
            ParameterKind::Normalized => Objects::PMeNa1(one(
                ioa,
                PMeNa1 {
                    nva: normalized(&self.value)?,
                    qpm: qpm()?,
                },
            )),
            ParameterKind::Scaled => Objects::PMeNb1(one(
                ioa,
                PMeNb1 {
                    sva: Sva::from_raw(parse(&self.value, "value")?),
                    qpm: qpm()?,
                },
            )),
            ParameterKind::Float => Objects::PMeNc1(one(
                ioa,
                PMeNc1 {
                    r32: R32::from_raw(parse(&self.value, "value")?),
                    qpm: qpm()?,
                },
            )),
            ParameterKind::Activation => Objects::PAcNa1(one(
                ioa,
                PAcNa1 {
                    qpa: Qpa {
                        value: parse(&self.qualifier, "QPA")?,
                    },
                },
            )),
        })
    }

    fn monitor_objects(&self, ioa: u32) -> Result<Objects, String> {
        let qds = self.quality;
        let t = self.time_tag;
        Ok(match self.monitor {
            MonitorKind::Single => {
                let siq = Siq::with_quality(self.on, qds);
                tagged!(t, ioa, MSpTb1, MSpNa1 { siq })
            }
            MonitorKind::Double => {
                let diq = Diq::with_quality(self.double, qds);
                tagged!(t, ioa, MDpTb1, MDpNa1 { diq })
            }
            MonitorKind::Step => {
                let value: i8 = parse(&self.value, "value")?;
                if !(-64..=63).contains(&value) {
                    return Err("step position: -64 to 63".into());
                }
                let vti = Vti {
                    value,
                    transient: false,
                };
                tagged!(t, ioa, MStTb1, MStNa1 { vti, qds })
            }
            MonitorKind::Bitstring => {
                let bsi = Bsi {
                    bits: hex_u32(&self.value)?,
                };
                tagged!(t, ioa, MBoTb1, MBoNa1 { bsi, qds })
            }
            MonitorKind::Normalized => {
                let nva = normalized(&self.value)?;
                tagged!(t, ioa, MMeTd1, MMeNa1 { nva, qds })
            }
            MonitorKind::Scaled => {
                let sva = Sva::from_raw(parse(&self.value, "value")?);
                tagged!(t, ioa, MMeTe1, MMeNb1 { sva, qds })
            }
            MonitorKind::Float => {
                let r32 = R32::from_raw(parse(&self.value, "value")?);
                tagged!(t, ioa, MMeTf1, MMeNc1 { r32, qds })
            }
            MonitorKind::Counter => {
                let bcr = Bcr {
                    reading: parse(&self.value, "value")?,
                    sequence: 0,
                    carry: false,
                    adjusted: false,
                    invalid: qds.invalid,
                };
                tagged!(t, ioa, MItTb1, MItNa1 { bcr })
            }
        })
    }
}

/// `Some(positive)` when `asdu` answers a command of `type_id` to `ioa`: its
/// activation confirmation, or its mirror with cause 44 to 47.
pub fn confirmation(asdu: &[u8], sizes: AddressSizes, type_id: u8, ioa: u32) -> Option<bool> {
    let header = Header::parse_with(asdu, sizes).ok()?;
    let objects = asdu.get(sizes.header_len()..)?;
    let answer = matches!(
        header.cause,
        Cause::ActivationCon
            | Cause::UnknownTypeId
            | Cause::UnknownCause
            | Cause::UnknownCommonAddress
            | Cause::UnknownInformationObjectAddress
    );
    let matches = answer
        && header.type_id.to_u8() == type_id
        && iec104::asdu::read_ioa_with(objects, sizes).ok()? == ioa;
    matches.then_some(header.cause == Cause::ActivationCon && !header.negative)
}

/// Sets a time-tagged command's CP56 time tag, its last seven octets, to
/// now: an execute held for its select's confirmation would otherwise go out
/// with the select's time, and stations refuse commands older than a limit.
pub fn retime(asdu: &mut Vec<u8>) -> Result<(), String> {
    let tagged = iec104::asdu::TypeId::CScTa1.to_u8()..=iec104::asdu::TypeId::CBoTa1.to_u8();
    if !asdu.first().is_some_and(|t| tagged.contains(t)) {
        return Ok(());
    }
    let at = asdu.len().checked_sub(7).ok_or("command too short")?;
    let time = now()?;
    asdu.truncate(at);
    time.encode(asdu);
    Ok(())
}

/// A station interrogation (QOI 20, cause act) to the broadcast common
/// address, which 5-101 cl.7.2.4 allows for `C_IC_NA_1`: the station
/// answers with its own common address.
pub fn find_station(ctx: &Context) -> Result<Vec<u8>, String> {
    let broadcast = Context {
        common_address: ctx.sizes.broadcast_common_address(),
        ..*ctx
    };
    Forms {
        tab: Tab::System,
        system: SystemKind::Interrogation,
        qualifier: "20".into(),
        ..Forms::default()
    }
    .build(&broadcast)
}

/// The common address of a received ASDU when it is not the configured one
/// (nor the broadcast address).
pub const fn other_station(configured: u16, received: u16, sizes: AddressSizes) -> Option<u16> {
    if received == configured || received == sizes.broadcast_common_address() {
        None
    } else {
        Some(received)
    }
}

/// `Nva::from_scaled` clamps silently outside the `NVA` range (5-101
/// cl.7.2.6.6), so a value it cannot carry is refused here instead.
pub fn normalized(text: &str) -> Result<Nva, String> {
    let value: f64 = parse(text, "value")?;
    if !(-1.0..1.0).contains(&value) {
        return Err("normalized value: from -1 to just under 1".into());
    }
    Ok(Nva::from_scaled(value))
}

pub fn hex_u32(text: &str) -> Result<u32, String> {
    let digits = text
        .trim()
        .trim_start_matches("0x")
        .trim_start_matches("0X");
    u32::from_str_radix(digits, 16).map_err(|_| "bits: not a hex number".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use iec104::asdu::{TypeId, decode};

    const CTX: Context = Context {
        sizes: AddressSizes::CONVENTIONAL,
        common_address: 7,
        originator: 0,
    };

    fn built(forms: &mut Forms) -> Asdu {
        decode(&forms.build(&CTX).unwrap()).unwrap()
    }

    #[test]
    fn single_command_select() {
        let mut forms = Forms {
            ioa: "4001".into(),
            select: true,
            ..Forms::default()
        };
        let asdu = built(&mut forms);
        assert_eq!(asdu.header.type_id, TypeId::CScNa1);
        assert_eq!(asdu.header.common_address, 7);
        assert_eq!(asdu.header.cause, Cause::Activation);
        let Objects::CScNa1(objects) = asdu.objects else {
            panic!()
        };
        assert_eq!(objects[0].ioa, 4001);
        assert!(objects[0].value.sco.scs && objects[0].value.sco.qoc.select);
    }

    #[test]
    fn read_is_a_request() {
        let mut forms = Forms {
            tab: Tab::System,
            system: SystemKind::Read,
            ioa: "101".into(),
            ..Forms::default()
        };
        forms.cause = forms.default_cause();
        let asdu = built(&mut forms);
        assert_eq!(asdu.header.type_id, TypeId::CRdNa1);
        assert_eq!(asdu.header.cause, Cause::Request);
    }

    #[test]
    fn every_kind_encodes() {
        let mut forms = Forms {
            value: "0".into(),
            qualifier: "1".into(),
            ..Forms::default()
        };
        for (kind, _) in PROCESS {
            for time_tag in [false, true] {
                forms.tab = Tab::Process;
                forms.process = kind;
                forms.time_tag = time_tag;
                assert!(forms.build(&CTX).is_ok(), "{kind:?}");
            }
        }
        for (kind, _) in SYSTEM {
            forms.tab = Tab::System;
            forms.system = kind;
            assert!(forms.build(&CTX).is_ok(), "{kind:?}");
        }
        for (kind, _) in PARAMETER {
            forms.tab = Tab::Parameter;
            forms.parameter = kind;
            assert!(forms.build(&CTX).is_ok(), "{kind:?}");
        }
        for (kind, _) in MONITOR {
            for time_tag in [false, true] {
                forms.tab = Tab::Monitor;
                forms.monitor = kind;
                forms.time_tag = time_tag;
                assert!(forms.build(&CTX).is_ok(), "{kind:?}");
            }
        }
    }

    #[test]
    fn interrogation_bytes() {
        let mut forms = Forms {
            tab: Tab::System,
            qualifier: "20".into(),
            ..Forms::default()
        };
        let mut ctx = CTX;
        ctx.common_address = 1;
        assert_eq!(
            forms.build(&ctx).unwrap(),
            [0x64, 0x01, 0x06, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x14]
        );
    }

    #[test]
    fn finds_the_station_by_broadcast() {
        assert_eq!(
            find_station(&CTX).unwrap(),
            [0x64, 0x01, 0x06, 0x00, 0xFF, 0xFF, 0x00, 0x00, 0x00, 0x14]
        );
        let narrow = Context {
            sizes: AddressSizes::new(1, 3, true).unwrap(),
            ..CTX
        };
        assert_eq!(
            find_station(&narrow).unwrap(),
            [0x64, 0x01, 0x06, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x14]
        );
    }

    #[test]
    fn hints_another_station() {
        let wide = AddressSizes::CONVENTIONAL;
        let narrow = AddressSizes::new(1, 3, true).unwrap();
        assert_eq!(other_station(1, 5, wide), Some(5));
        assert_eq!(other_station(5, 5, wide), None);
        assert_eq!(other_station(1, 0xFFFF, wide), None);
        assert_eq!(other_station(1, 0xFF, wide), Some(0xFF));
        assert_eq!(other_station(1, 0xFF, narrow), None);
    }

    #[test]
    fn select_then_execute() {
        let mut forms = Forms {
            ioa: "4001".into(),
            ..Forms::default()
        };
        let Ok(Action::SelectExecute {
            select,
            execute,
            type_id,
            ioa,
        }) = forms.select_execute(&CTX)
        else {
            panic!()
        };
        assert_eq!((type_id, ioa), (45, 4001));
        assert!(!forms.select);
        let Objects::CScNa1(s) = decode(&select).unwrap().objects else {
            panic!()
        };
        let Objects::CScNa1(e) = decode(&execute).unwrap().objects else {
            panic!()
        };
        assert!(s[0].value.sco.qoc.select && !e[0].value.sco.qoc.select);

        let mut confirmed = select;
        confirmed[2] = 7;
        assert_eq!(confirmation(&confirmed, CTX.sizes, 45, 4001), Some(true));
        assert_eq!(confirmation(&confirmed, CTX.sizes, 45, 4002), None);
        confirmed[2] = 0x47;
        assert_eq!(confirmation(&confirmed, CTX.sizes, 45, 4001), Some(false));
        confirmed[2] = 0x6F;
        assert_eq!(confirmation(&confirmed, CTX.sizes, 45, 4001), Some(false));
        confirmed[2] = 6;
        assert_eq!(confirmation(&confirmed, CTX.sizes, 45, 4001), None);
    }

    #[test]
    fn execute_gets_a_fresh_time_tag() {
        let mut forms = Forms {
            ioa: "4001".into(),
            time_tag: true,
            ..Forms::default()
        };
        let Ok(Action::SelectExecute { mut execute, .. }) = forms.select_execute(&CTX) else {
            panic!()
        };
        let at = execute.len() - 7;
        execute[at..].fill(0);
        retime(&mut execute).unwrap();
        let Objects::CScTa1(e) = decode(&execute).unwrap().objects else {
            panic!()
        };
        let age = SystemTime::now()
            .duration_since(e[0].value.time.to_system_time().unwrap())
            .unwrap();
        assert!(age.as_secs() < 5, "{age:?}");

        forms.time_tag = false;
        let untagged = forms.build(&CTX).unwrap();
        let mut same = untagged.clone();
        retime(&mut same).unwrap();
        assert_eq!(same, untagged);
    }

    #[test]
    fn normalized_values_stay_in_range() {
        for ok in ["-1", "0", "0.5", "0.99996"] {
            assert!(normalized(ok).is_ok(), "{ok}");
        }
        for bad in ["1", "5", "-1.5", "nan", "inf"] {
            assert!(normalized(bad).is_err(), "{bad}");
        }
        let mut forms = Forms {
            tab: Tab::Process,
            process: ProcessKind::Normalized,
            value: "5".into(),
            ..Forms::default()
        };
        assert!(forms.build(&CTX).is_err(), "not clamped to 0.99997");
        forms.tab = Tab::Parameter;
        forms.parameter = ParameterKind::Normalized;
        forms.qualifier = "1".into();
        assert!(forms.build(&CTX).is_err());
        forms.tab = Tab::Monitor;
        forms.monitor = MonitorKind::Normalized;
        assert!(forms.build(&CTX).is_err());
    }

    #[test]
    fn rejects_bad_input() {
        let mut forms = Forms {
            ioa: "x".into(),
            ..Forms::default()
        };
        assert!(forms.build(&CTX).is_err());
    }
}
