use std::path::Path;
use std::time::Duration;

use iec104::asdu::AddressSizes;
use iec104::params::LinkParams;
use iec104_tls::{Identity, TlsConfig, certificates_from_pem_file};
use serde::{Deserialize, Serialize};

use crate::simulator::SimPoint;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub host: String,
    pub port: u16,
    pub common_address: u16,
    pub originator: u8,
    /// Cause of transmission: 1 or 2 octets (2 carries the originator address).
    pub cot_octets: u8,
    pub ca_octets: u8,
    pub ioa_octets: u8,
    pub k: u16,
    pub w: u16,
    pub t0: u64,
    pub t1: u64,
    pub t2: u64,
    pub t3: u64,
    pub auto_start: bool,
    pub tls: Tls,
    pub station: Vec<SimPoint>,
    /// Simulator: report a commanded point's new value as return information.
    pub echo: bool,
    /// Simulator: commands need a select before the execute.
    pub select_before_execute: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Tls {
    pub enabled: bool,
    pub certificate: String,
    pub key: String,
    pub ca: String,
    pub verify_server_name: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let link = LinkParams::default();
        Self {
            host: "127.0.0.1".into(),
            port: link.port,
            common_address: 1,
            originator: 0,
            cot_octets: 2,
            ca_octets: 2,
            ioa_octets: 3,
            k: link.k,
            w: link.w,
            t0: link.t0.as_secs(),
            t1: link.t1.as_secs(),
            t2: link.t2.as_secs(),
            t3: link.t3.as_secs(),
            auto_start: true,
            tls: Tls::default(),
            station: (1..=3).map(SimPoint::new).collect(),
            echo: true,
            select_before_execute: false,
        }
    }
}

impl Settings {
    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }

    pub fn sizes(&self) -> Result<AddressSizes, String> {
        AddressSizes::new(self.ca_octets, self.ioa_octets, self.cot_octets == 2)
            .map_err(|e| format!("address sizes: {e:?}"))
    }

    pub fn link(&self) -> Result<LinkParams, String> {
        let params = LinkParams {
            port: self.port,
            k: self.k,
            w: self.w,
            t0: Duration::from_secs(self.t0),
            t1: Duration::from_secs(self.t1),
            t2: Duration::from_secs(self.t2),
            t3: Duration::from_secs(self.t3),
        };
        params
            .validate()
            .map_err(|e| format!("link parameters: {e:?}"))?;
        Ok(params)
    }

    pub fn tls_config(&self) -> Result<TlsConfig, String> {
        let identity = Identity::from_pem_files(&self.tls.certificate, &self.tls.key)
            .map_err(|e| format!("TLS identity: {e}"))?;
        let roots = certificates_from_pem_file(&self.tls.ca).map_err(|e| format!("TLS CA: {e}"))?;
        TlsConfig::builder(identity, roots)
            .verify_server_name(self.tls.verify_server_name)
            .build()
            .map_err(|e| format!("TLS: {e}"))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        toml::from_str(&text).map_err(|e| e.to_string())
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_round_trip() {
        let settings = Settings::default();
        assert!(settings.link().is_ok());
        assert_eq!(settings.sizes().unwrap(), AddressSizes::CONVENTIONAL);
        let text = toml::to_string_pretty(&settings).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), settings);
    }

    #[test]
    fn rejects_bad_sizes() {
        let settings = Settings {
            ioa_octets: 4,
            ..Settings::default()
        };
        assert!(settings.sizes().is_err());
    }
}
