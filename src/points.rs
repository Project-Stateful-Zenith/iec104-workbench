use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use chrono::{DateTime, Local};
use iec104::asdu::{self, AddressSizes, Quality, TimeTag, Value};

use crate::telegram::cause_name;

pub struct Row {
    pub type_name: &'static str,
    pub value: String,
    pub quality: String,
    pub time: String,
    pub cause: String,
    pub updates: u64,
    pub received: DateTime<Local>,
}

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
                    quality: String::new(),
                    time: String::new(),
                    cause: String::new(),
                    updates: 0,
                    received: at,
                });
            row.type_name = header.type_id.name();
            row.value = value_text(&point.value);
            row.quality = point.quality.map(quality_text).unwrap_or_default();
            row.time = point.time.map(time_text).unwrap_or_default();
            row.cause = cause_name(header.cause);
            row.updates = row.updates.saturating_add(1);
            row.received = at;
        }
    }

    pub fn save_csv(&self, path: &Path) -> Result<(), String> {
        let mut text = String::from("ca,ioa,type,value,quality,time,cause,updates,received\n");
        for ((ca, ioa), r) in &self.rows {
            let _ = writeln!(
                text,
                "{ca},{ioa},{},\"{}\",{},{},{},{},{}",
                r.type_name,
                r.value.replace('"', "\"\""),
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
}
