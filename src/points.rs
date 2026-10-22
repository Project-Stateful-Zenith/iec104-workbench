use std::collections::{BTreeMap, VecDeque};
use std::fmt::Write as _;
use std::path::Path;

use chrono::{DateTime, Datelike, Local, Timelike};
use iec104::asdu::{self, AddressSizes, Quality, TimeTag, Value};
use serde::{Deserialize, Serialize};

use crate::telegram::cause_name;

pub struct Row {
    pub type_name: &'static str,
    pub value: String,
    /// The latest value as a number, before any scale.
    pub number: Option<f64>,
    pub quality: String,
    pub time: String,
    pub cause: String,
    pub updates: u64,
    pub received: DateTime<Local>,
    /// The samples charted, `[seconds, value]`; `None` while not charted.
    pub history: Option<VecDeque<[f64; 2]>>,
}

/// The most samples kept per charted point.
pub const MAX_SAMPLES: usize = 10_000;

/// Keyed by (common address, object address).
#[derive(Default)]
pub struct Points {
    pub rows: BTreeMap<(u16, u32), Row>,
}

impl Points {
    pub fn apply(&mut self, asdu: &[u8], sizes: AddressSizes, at: DateTime<Local>) {
        let Ok(decoded) = asdu::decode_with(asdu, sizes) else {
            return;
        };
        let header = decoded.header;
        for point in decoded.objects.points() {
            let row = self
                .rows
                .entry((header.common_address, point.ioa))
                .or_insert_with(|| Row {
                    type_name: "",
                    value: String::new(),
                    number: None,
                    quality: String::new(),
                    time: String::new(),
                    cause: String::new(),
                    updates: 0,
                    received: at,
                    history: None,
                });
            row.type_name = header.type_id.name();
            row.value = value_text(&point.value);
            row.number = numeric(&point.value);
            row.quality = point.quality.map(quality_text).unwrap_or_default();
            row.time = point.time.map(time_text).unwrap_or_default();
            row.cause = cause_name(header.cause);
            row.updates = row.updates.saturating_add(1);
            row.received = at;
            if let (Some(history), Some(value)) = (&mut row.history, row.number) {
                if history.len() >= MAX_SAMPLES {
                    history.pop_front();
                }
                history.push_back([seconds(at), value]);
            }
        }
    }

    pub fn save_csv(&self, path: &Path, scales: &[Scale]) -> Result<(), String> {
        let mut text =
            String::from("ca,ioa,type,value,scaled,quality,time,cause,updates,received\n");
        for ((ca, ioa), r) in &self.rows {
            let _ = writeln!(
                text,
                "{ca},{ioa},{},\"{}\",{},{},{},{},{},{}",
                r.type_name,
                r.value.replace('"', "\"\""),
                scaled_text(scales, *ca, *ioa, r.number),
                r.quality,
                r.time,
                r.cause,
                r.updates,
                r.received.format("%F %T%.3f")
            );
        }
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Multiply,
    Divide,
    Add,
    Subtract,
}

impl Op {
    pub const ALL: [Self; 4] = [Self::Multiply, Self::Divide, Self::Add, Self::Subtract];

    pub const fn symbol(self) -> &'static str {
        match self {
            Self::Multiply => "×",
            Self::Divide => "÷",
            Self::Add => "+",
            Self::Subtract => "−",
        }
    }
}

/// How one point's value is shown: `raw op factor`, rounded to `decimals`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Scale {
    pub ca: u16,
    pub ioa: u32,
    pub op: Op,
    pub factor: f64,
    pub decimals: u8,
}

impl Scale {
    /// `None` when dividing by zero.
    pub fn apply(&self, raw: f64) -> Option<f64> {
        match self.op {
            Op::Multiply => Some(raw * self.factor),
            Op::Divide => (self.factor != 0.0).then(|| raw / self.factor),
            Op::Add => Some(raw + self.factor),
            Op::Subtract => Some(raw - self.factor),
        }
    }

    pub fn label(&self) -> String {
        format!("{} {}", self.op.symbol(), self.factor)
    }
}

pub fn find(scales: &[Scale], ca: u16, ioa: u32) -> Option<&Scale> {
    scales.iter().find(|s| s.ca == ca && s.ioa == ioa)
}

/// The scaled value, empty for unscaled or non-numeric points.
pub fn scaled_text(scales: &[Scale], ca: u16, ioa: u32, raw: Option<f64>) -> String {
    find(scales, ca, ioa)
        .zip(raw)
        .and_then(|(s, raw)| {
            s.apply(raw)
                .map(|v| format!("{v:.*}", usize::from(s.decimals)))
        })
        .unwrap_or_default()
}

/// The chart's x: seconds since 0001-01-01 local time, kept continuous over
/// midnight and exact in an f64.
pub fn seconds(at: DateTime<Local>) -> f64 {
    f64::from(at.num_days_from_ce()).mul_add(
        86_400.0,
        f64::from(at.num_seconds_from_midnight()) + f64::from(at.nanosecond()) / 1e9,
    )
}

/// The value as a number to chart; double points as their bits 0 to 3.
pub fn numeric(value: &Value) -> Option<f64> {
    match *value {
        Value::Single(on) => Some(f64::from(u8::from(on))),
        Value::Double(state) => Some(f64::from(state.to_bits())),
        Value::StepPosition { value, .. } => Some(f64::from(value)),
        Value::Normalized(v) => Some(v),
        Value::Scaled(v) => Some(f64::from(v)),
        Value::Float(v) => Some(f64::from(v)),
        Value::Counter { reading, .. } => Some(f64::from(reading)),
        _ => None,
    }
}

pub fn value_text(value: &Value) -> String {
    match *value {
        Value::Single(on) => if on { "ON" } else { "OFF" }.into(),
        Value::Double(state) => format!("{state:?}"),
        Value::StepPosition { value, transient } => {
            format!("{value}{}", if transient { " transient" } else { "" })
        }
        Value::Bitstring(bits) => format!("0x{bits:08X}"),
        Value::Normalized(v) => format!("{v:.5}"),
        Value::Scaled(v) => v.to_string(),
        Value::Float(v) => v.to_string(),
        Value::Counter {
            reading,
            sequence,
            carry,
            adjusted,
        } => format!(
            "{reading} sq={sequence}{}{}",
            if carry { " CY" } else { "" },
            if adjusted { " CA" } else { "" }
        ),
        other => format!("{other:?}"),
    }
}

pub fn quality_text(q: Quality) -> String {
    [
        (q.invalid, "IV"),
        (q.not_topical, "NT"),
        (q.substituted, "SB"),
        (q.blocked, "BL"),
        (q.overflow.unwrap_or(false), "OV"),
    ]
    .into_iter()
    .filter_map(|(on, name)| on.then_some(name))
    .collect::<Vec<_>>()
    .join(" ")
}

pub fn time_text(time: TimeTag) -> String {
    match time {
        TimeTag::Cp56(t) => format!(
            "20{:02}-{:02}-{:02} {:02}:{:02}:{:02}.{:03}{}{}",
            t.years,
            t.months,
            t.day_of_month,
            t.hours,
            t.minutes,
            t.milliseconds / 1000,
            t.milliseconds % 1000,
            if t.summer_time { " SU" } else { "" },
            if t.invalid { " IV" } else { "" }
        ),
        TimeTag::Cp24(t) => format!(
            "xx:{:02}:{:02}.{:03}{}",
            t.minutes,
            t.milliseconds / 1000,
            t.milliseconds % 1000,
            if t.invalid { " IV" } else { "" }
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_latest_value() {
        // M_ME_NB_1, CA 12, IOA 0x3010, value 1 then 2.
        let mut asdu = [
            0x0B, 0x01, 0x03, 0x00, 0x0C, 0x00, 0x10, 0x30, 0x00, 0x01, 0x00, 0x00,
        ];
        let mut points = Points::default();
        points.apply(&asdu, AddressSizes::CONVENTIONAL, Local::now());
        asdu[9] = 2;
        points.apply(&asdu, AddressSizes::CONVENTIONAL, Local::now());
        let row = &points.rows[&(12, 0x3010)];
        assert_eq!(row.value, "2");
        assert_eq!(row.updates, 2);
        assert_eq!(row.cause, "spont");
    }

    #[test]
    fn charts_only_marked_points_and_keeps_the_newest_samples() {
        // M_ME_NB_1, CA 12, IOA 0x3010.
        let mut asdu = [
            0x0B, 0x01, 0x03, 0x00, 0x0C, 0x00, 0x10, 0x30, 0x00, 0x01, 0x00, 0x00,
        ];
        let mut points = Points::default();
        let at = Local::now();
        points.apply(&asdu, AddressSizes::CONVENTIONAL, at);
        let row = points.rows.get_mut(&(12, 0x3010)).unwrap();
        assert!(row.history.is_none());
        row.history = Some(VecDeque::new());
        for value in 0..=MAX_SAMPLES {
            asdu[9..11].copy_from_slice(&u16::try_from(value).unwrap().to_le_bytes());
            points.apply(&asdu, AddressSizes::CONVENTIONAL, at);
        }
        let history = points.rows[&(12, 0x3010)].history.as_ref().unwrap();
        assert_eq!(history.len(), MAX_SAMPLES);
        assert_eq!(history.front().unwrap()[1], 1.0);
        assert_eq!(history.back().unwrap(), &[seconds(at), 10_000.0]);
    }

    #[test]
    fn maps_values_to_numbers() {
        use iec104::asdu::DoublePointState;
        assert_eq!(numeric(&Value::Single(true)), Some(1.0));
        assert_eq!(numeric(&Value::Double(DoublePointState::On)), Some(2.0));
        assert_eq!(
            numeric(&Value::StepPosition {
                value: -5,
                transient: true
            }),
            Some(-5.0)
        );
        assert_eq!(numeric(&Value::Scaled(-300)), Some(-300.0));
        assert_eq!(numeric(&Value::Float(1.5)), Some(1.5));
        assert_eq!(numeric(&Value::Bitstring(1)), None);
    }

    #[test]
    fn scales_and_rounds() {
        let mut scale = Scale {
            ca: 1,
            ioa: 100,
            op: Op::Divide,
            factor: 1000.0,
            decimals: 2,
        };
        let scales = [scale.clone()];
        assert_eq!(scaled_text(&scales, 1, 100, Some(1234.0)), "1.23");
        assert_eq!(scaled_text(&scales, 1, 101, Some(1234.0)), "");
        assert_eq!(scaled_text(&scales, 1, 100, None), "");
        scale.factor = 0.0;
        assert_eq!(scale.apply(1.0), None);
        scale.factor = 2.0;
        for (op, expected) in [(Op::Multiply, 20.0), (Op::Add, 12.0), (Op::Subtract, 8.0)] {
            scale.op = op;
            assert_eq!(scale.apply(10.0), Some(expected));
        }
    }

    #[test]
    fn seconds_run_on_over_midnight() {
        use chrono::TimeZone;
        let before = Local.with_ymd_and_hms(2026, 1, 1, 23, 59, 59).unwrap();
        let after = Local.with_ymd_and_hms(2026, 1, 2, 0, 0, 1).unwrap();
        assert_eq!(seconds(after) - seconds(before), 2.0);
    }
}
