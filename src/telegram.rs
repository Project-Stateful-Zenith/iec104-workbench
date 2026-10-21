use std::fmt::Write as _;
use std::path::Path;

use chrono::{DateTime, Local, TimeZone};
use iec104::apci::{self, Apci};
use iec104::asdu::{self, AddressSizes, Header};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dir {
    Tx,
    Rx,
}

impl Dir {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tx => "TX",
            Self::Rx => "RX",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Link,
    Monitor,
    Process,
    System,
    Parameter,
    File,
    Other,
}

impl Category {
    pub const ALL: [Self; 7] = [
        Self::Link,
        Self::Monitor,
        Self::Process,
        Self::System,
        Self::Parameter,
        Self::File,
        Self::Other,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Link => "S/U frames",
            Self::Monitor => "Monitor",
            Self::Process => "Process commands",
            Self::System => "System commands",
            Self::Parameter => "Parameters",
            Self::File => "File transfer",
            Self::Other => "Other",
        }
    }

    const fn of(type_id: u8) -> Self {
        match type_id {
            1..=44 | 70 => Self::Monitor,
            45..=69 => Self::Process,
            100..=109 => Self::System,
            110..=119 => Self::Parameter,
            120..=127 => Self::File,
            _ => Self::Other,
        }
    }
}

pub struct Telegram {
    pub at: DateTime<Local>,
    pub dir: Dir,
    pub bytes: Vec<u8>,
    pub category: Category,
    pub header: Option<Header>,
    pub ioas: Vec<u32>,
    pub summary: String,
    /// The address sizes the telegram was read with.
    pub sizes: AddressSizes,
}

impl Telegram {
    pub fn new(at: DateTime<Local>, dir: Dir, bytes: Vec<u8>, sizes: AddressSizes) -> Self {
        let summary = iec104::diag::apdu(&bytes, sizes).unwrap_or_else(|e| format!("{e:?}"));
        let mut header = None;
        let mut ioas = Vec::new();
        let mut category = Category::Link;
        if let Ok((apdu, _)) = apci::parse(&bytes)
            && apdu.apci.is_i_format()
        {
            category = Category::Other;
            if let Ok(parsed) = Header::parse_with(apdu.asdu, sizes) {
                category = Category::of(parsed.type_id.to_u8());
                header = Some(parsed);
                ioas = object_addresses(apdu.asdu, sizes);
            }
        }
        Self {
            at,
            dir,
            bytes,
            category,
            header,
            ioas,
            summary,
            sizes,
        }
    }

    pub fn asdu(&self) -> Option<&[u8]> {
        apci::parse(&self.bytes)
            .ok()
            .filter(|(apdu, _)| apdu.apci.is_i_format())
            .map(|(apdu, _)| apdu.asdu)
    }

    pub fn details(&self) -> String {
        let sizes = self.sizes;
        let mut out = String::new();
        let _ = writeln!(
            out,
            "{} {}  {} octets",
            self.dir.label(),
            self.at.format("%F %T%.3f"),
            self.bytes.len()
        );
        let _ = writeln!(out, "{}\n", hex(&self.bytes));
        let apdu = match apci::parse(&self.bytes) {
            Ok((apdu, _)) => apdu,
            Err(e) => {
                let _ = writeln!(out, "APCI: {e:?}");
                return out;
            }
        };
        let _ = writeln!(out, "APCI");
        let _ = writeln!(out, "  control  {}", hex(&apdu.apci.control_field()));
        match apdu.apci {
            Apci::I { ns, nr } => {
                let _ = writeln!(out, "  I-format  N(S)={}  N(R)={}", ns.get(), nr.get());
            }
            Apci::S { nr } => {
                let _ = writeln!(out, "  S-format  N(R)={}", nr.get());
            }
            Apci::U(function) => {
                let _ = writeln!(out, "  U-format  {function:?}");
            }
        }
        if !apdu.apci.is_i_format() {
            return out;
        }
        let _ = writeln!(out, "\nASDU  {}", hex(apdu.asdu));
        match Header::parse_with(apdu.asdu, sizes) {
            Ok(h) => {
                let _ = writeln!(
                    out,
                    "  type      {} <{}>",
                    h.type_id.name(),
                    h.type_id.to_u8()
                );
                let _ = writeln!(
                    out,
                    "  SQ        {}  objects {}",
                    u8::from(h.sequence),
                    h.count
                );
                let _ = writeln!(
                    out,
                    "  cause     {} <{}>{}{}",
                    cause_name(h.cause),
                    h.cause.to_u8(),
                    if h.negative { " negative" } else { "" },
                    if h.test { " test" } else { "" }
                );
                let _ = writeln!(out, "  originator {}", h.originator);
                let _ = writeln!(out, "  CA        {}", h.common_address);
            }
            Err(e) => {
                let _ = writeln!(out, "  header: {e:?}");
                return out;
            }
        }
        match asdu::decode_with(apdu.asdu, sizes) {
            Ok(decoded) => {
                let _ = writeln!(out, "\nInformation objects\n{:#?}", decoded.objects);
            }
            Err(e) => {
                let _ = writeln!(out, "\nobjects: {e:?}");
            }
        }
        out
    }
}

fn object_addresses(asdu: &[u8], sizes: AddressSizes) -> Vec<u32> {
    asdu::decode_with(asdu, sizes).map_or_else(
        |_| Vec::new(),
        |decoded| decoded.objects.validity().iter().map(|v| v.ioa).collect(),
    )
}

pub fn cause_name(cause: asdu::Cause) -> String {
    match cause {
        asdu::Cause::UnknownTypeId => "unk type".into(),
        asdu::Cause::UnknownCause => "unk cause".into(),
        asdu::Cause::UnknownCommonAddress => "unk CA".into(),
        asdu::Cause::UnknownInformationObjectAddress => "unk IOA".into(),
        _ if cause.mnemonic().is_empty() => format!("{cause:?}"),
        _ => cause.mnemonic().into(),
    }
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().saturating_mul(3));
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let _ = write!(out, "{b:02X}");
    }
    out
}

pub fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    let digits: String = text
        .replace("0x", "")
        .replace("0X", "")
        .chars()
        .filter(|c| !c.is_whitespace() && *c != ',')
        .collect();
    if !digits.len().is_multiple_of(2) {
        return Err("odd number of hex digits".into());
    }
    digits
        .as_bytes()
        .chunks(2)
        .map(|pair| {
            std::str::from_utf8(pair)
                .ok()
                .and_then(|s| u8::from_str_radix(s, 16).ok())
                .ok_or_else(|| format!("not hex: {}", String::from_utf8_lossy(pair)))
        })
        .collect()
}

/// The most telegrams kept: about 300 MB, a day at a few frames a second.
pub const MAX_TELEGRAMS: usize = 1_000_000;

/// Makes room for one more telegram once `list` holds `max`: drops the oldest
/// half and returns how many went, so indices into the list can follow.
pub fn make_room<T>(list: &mut Vec<T>, max: usize) -> usize {
    if list.len() < max {
        return 0;
    }
    let dropped = list.len().saturating_sub(max / 2);
    list.drain(..dropped);
    dropped
}

/// Which telegrams the list shows. Empty text fields match everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub ca: String,
    pub ioa: String,
    pub types: String,
    pub causes: String,
    pub tx: bool,
    pub rx: bool,
    pub categories: [bool; 7],
}

impl Default for Filter {
    fn default() -> Self {
        Self {
            ca: String::new(),
            ioa: String::new(),
            types: String::new(),
            causes: String::new(),
            tx: true,
            rx: true,
            categories: [true; 7],
        }
    }
}

pub struct Matcher {
    ca: Option<u16>,
    ioa: Option<u32>,
    types: Vec<u8>,
    causes: Vec<u8>,
    tx: bool,
    rx: bool,
    categories: Vec<Category>,
}

impl Filter {
    /// Unparsable fields are ignored, as if empty.
    pub fn matcher(&self) -> Matcher {
        let list = |text: &str| -> Vec<u8> {
            text.split(',')
                .filter_map(|s| s.trim().parse().ok())
                .collect()
        };
        Matcher {
            ca: self.ca.trim().parse().ok(),
            ioa: self.ioa.trim().parse().ok(),
            types: list(&self.types),
            causes: list(&self.causes),
            tx: self.tx,
            rx: self.rx,
            categories: Category::ALL
                .into_iter()
                .zip(self.categories)
                .filter_map(|(c, on)| on.then_some(c))
                .collect(),
        }
    }
}

impl Matcher {
    pub fn matches(&self, t: &Telegram) -> bool {
        let dir = match t.dir {
            Dir::Tx => self.tx,
            Dir::Rx => self.rx,
        };
        if !dir || !self.categories.contains(&t.category) {
            return false;
        }
        let narrowed = self.ca.is_some()
            || self.ioa.is_some()
            || !self.types.is_empty()
            || !self.causes.is_empty();
        let Some(h) = t.header else {
            return !narrowed;
        };
        self.ca.is_none_or(|ca| ca == h.common_address)
            && self.ioa.is_none_or(|ioa| t.ioas.contains(&ioa))
            && (self.types.is_empty() || self.types.contains(&h.type_id.to_u8()))
            && (self.causes.is_empty() || self.causes.contains(&h.cause.to_u8()))
    }
}

/// Writes one line per telegram: `<unix ms> <TX|RX> <hex>`, and before the
/// first and wherever they change, the address sizes the telegrams after it
/// were read with: `# sizes <CA octets> <IOA octets> <COT octets>`.
pub fn save(path: &Path, telegrams: &[Telegram]) -> Result<(), String> {
    let mut text = String::new();
    let mut current = None;
    for t in telegrams {
        if current != Some(t.sizes) {
            current = Some(t.sizes);
            let _ = writeln!(
                text,
                "{SIZES} {} {} {}",
                t.sizes.common_address_octets(),
                t.sizes.information_object_address_octets(),
                if t.sizes.originator_address() { 2 } else { 1 }
            );
        }
        let _ = writeln!(
            text,
            "{} {} {}",
            t.at.timestamp_millis(),
            t.dir.label(),
            hex(&t.bytes)
        );
    }
    std::fs::write(path, text).map_err(|e| e.to_string())
}

const SIZES: &str = "# sizes";

/// Reads a recording [`save`] wrote. Telegrams before any `# sizes` line, as
/// in recordings from before it existed, are read with `sizes`.
pub fn load(path: &Path, mut sizes: AddressSizes) -> Result<Vec<Telegram>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let mut telegrams = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let at = |e: String| format!("line {}: {e}", n.saturating_add(1));
        if let Some(rest) = line.strip_prefix(SIZES) {
            sizes = parse_sizes(rest).map_err(at)?;
        } else if !line.trim().is_empty() {
            telegrams.push(parse_line(line, sizes).map_err(at)?);
        }
    }
    Ok(telegrams)
}

fn parse_sizes(text: &str) -> Result<AddressSizes, String> {
    let numbers: Vec<u8> = text
        .split_whitespace()
        .map(|n| n.parse().map_err(|_| format!("bad sizes: {text}")))
        .collect::<Result<_, _>>()?;
    let [ca, ioa, cot] = numbers[..] else {
        return Err(format!("bad sizes: {text}"));
    };
    AddressSizes::new(ca, ioa, cot == 2).map_err(|e| format!("{e:?}"))
}

fn parse_line(line: &str, sizes: AddressSizes) -> Result<Telegram, String> {
    let mut parts = line.splitn(3, ' ');
    let millis: i64 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or("bad timestamp")?;
    let dir = match parts.next() {
        Some("TX") => Dir::Tx,
        Some("RX") => Dir::Rx,
        _ => return Err("bad direction".into()),
    };
    let bytes = parse_hex(parts.next().unwrap_or_default())?;
    let at = Local
        .timestamp_millis_opt(millis)
        .single()
        .ok_or("bad timestamp")?;
    Ok(Telegram::new(at, dir, bytes, sizes))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZES: AddressSizes = AddressSizes::CONVENTIONAL;
    /// I-frame, `M_ME_NB_1` spontaneous, CA 12, IOA 0x3010.
    const MEASURED: [u8; 18] = [
        0x68, 0x10, 0x02, 0x00, 0x04, 0x00, 0x0B, 0x01, 0x03, 0x00, 0x0C, 0x00, 0x10, 0x30, 0x00,
        0x00, 0x01, 0x00,
    ];

    fn telegram(bytes: &[u8]) -> Telegram {
        Telegram::new(Local::now(), Dir::Rx, bytes.to_vec(), SIZES)
    }

    #[test]
    fn classifies_frames() {
        let t = telegram(&MEASURED);
        assert_eq!(t.category, Category::Monitor);
        assert_eq!(t.ioas, [0x3010]);
        assert!(t.details().contains("M_ME_NB_1"));
        let u = telegram(&[0x68, 0x04, 0x07, 0x00, 0x00, 0x00]);
        assert_eq!(u.category, Category::Link);
        assert_eq!(u.summary, "U STARTDT act");
    }

    #[test]
    fn filters() {
        let t = telegram(&MEASURED);
        let pass = |f: Filter| f.matcher().matches(&t);
        assert!(pass(Filter::default()));
        assert!(pass(Filter {
            ca: "12".into(),
            ioa: "12304".into(),
            ..Filter::default()
        }));
        assert!(!pass(Filter {
            ca: "13".into(),
            ..Filter::default()
        }));
        assert!(pass(Filter {
            types: "1, 11".into(),
            causes: "3".into(),
            ..Filter::default()
        }));
        assert!(!pass(Filter {
            rx: false,
            ..Filter::default()
        }));
    }

    #[test]
    fn a_recording_keeps_its_address_sizes() {
        // M_SP_NA_1 with a one-octet common address and a two-octet IOA:
        // header 6 octets less one, IOA 2 octets.
        let narrow = AddressSizes::new(1, 2, true).unwrap();
        let asdu = [0x01, 0x01, 0x03, 0x00, 0x07, 0x34, 0x12, 0x01];
        let mut bytes = vec![0x68, 0x0C, 0x00, 0x00, 0x00, 0x00];
        bytes.extend(asdu);
        let path = std::env::temp_dir().join("iec104-workbench-sizes.log");
        save(
            &path,
            &[Telegram::new(Local::now(), Dir::Rx, bytes, narrow)],
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let back = load(&path, SIZES).unwrap();
        let _ = std::fs::remove_file(&path);
        assert!(text.starts_with("# sizes 1 2 2\n"), "{text}");
        assert_eq!(back[0].sizes, narrow);
        assert_eq!(back[0].header.unwrap().common_address, 7);
        assert_eq!(back[0].ioas, [0x1234]);
    }

    #[test]
    fn keeps_the_newer_half_when_full() {
        let mut list: Vec<u32> = (0..10).collect();
        assert_eq!(make_room(&mut list, 11), 0);
        assert_eq!(list.len(), 10);
        assert_eq!(make_room(&mut list, 10), 5);
        assert_eq!(list, [5, 6, 7, 8, 9]);
    }

    #[test]
    fn hex_round_trip_and_file() {
        assert_eq!(
            parse_hex("0x68 04,07 00").unwrap(),
            [0x68, 0x04, 0x07, 0x00]
        );
        assert!(parse_hex("6").is_err());
        let dir = std::env::temp_dir().join("iec104-workbench-test.log");
        save(&dir, &[telegram(&MEASURED)]).unwrap();
        let back = load(&dir, SIZES).unwrap();
        let _ = std::fs::remove_file(&dir);
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].bytes, MEASURED);
    }
}
