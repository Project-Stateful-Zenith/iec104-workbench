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
    pub fn ui(&mut self, ui: &mut Ui, ctx: &Context) -> Option<Result<Vec<u8>, String>> {
        ui.horizontal_wrapped(|ui| {
            for (tab, label) in [
                (Tab::Process, "Process"),
                (Tab::System, "System"),
                (Tab::Parameter, "Parameter"),
                (Tab::Monitor, "Monitor"),
                (Tab::Manual, "Hex"),
            ] {
                if ui.selectable_value(&mut self.tab, tab, label).clicked() {
                    self.cause = match tab {
                        Tab::Monitor => 3,
                        _ => 6,
                    };
                    self.qualifier = self.default_qualifier().into();
                }
            }
        });
        ui.separator();
        if self.tab == Tab::Manual {
            return self.manual_ui(ui, ctx);
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
        ui.button("Send").clicked().then(|| self.build(ctx))
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
                if self.process != ProcessKind::Bitstring {
                    field(ui, label, &mut self.qualifier);
                    ui.checkbox(&mut self.select, "Select (else execute)");
                }
                ui.checkbox(&mut self.time_tag, "With CP56 time tag");
            }
            Tab::System => {
                let before = self.system;
                choice(ui, "system", &mut self.system, &SYSTEM);
                if self.system != before {
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
                let nva = Nva::from_scaled(parse(&self.value, "value")?);
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
                    nva: Nva::from_scaled(parse(&self.value, "value")?),
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
                let nva = Nva::from_scaled(parse(&self.value, "value")?);
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

fn hex_u32(text: &str) -> Result<u32, String> {
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
    fn every_kind_encodes() {
        let mut forms = Forms {
            value: "1".into(),
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
    fn rejects_bad_input() {
        let mut forms = Forms {
            ioa: "x".into(),
            ..Forms::default()
        };
        assert!(forms.build(&CTX).is_err());
    }
}
