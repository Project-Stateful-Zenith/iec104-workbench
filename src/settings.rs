use std::path::Path;
use std::time::Duration;

use iec104::asdu::AddressSizes;
use iec104::params::LinkParams;
use iec104_tls::{Identity, TlsConfig, Version, certificates_from_pem_file, crls_from_pem_file};
use serde::{Deserialize, Serialize};

use crate::simulator::{ServedFile, SimPoint};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(clippy::struct_excessive_bools, reason = "one field per checkbox")]
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
    pub served: Vec<ServedFile>,
    /// Simulator: report a commanded point's new value as return information.
    pub echo: bool,
    /// Simulator: commands need a select before the execute.
    pub select_before_execute: bool,
    /// Simulator: receive files the client uploads.
    pub accept_uploads: bool,
    /// Simulator: where received files are saved.
    pub upload_folder: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
#[expect(clippy::struct_excessive_bools, reason = "independent switches")]
pub struct Tls {
    pub enabled: bool,
    pub certificate: String,
    pub key: String,
    pub ca: String,
    pub verify_server_name: bool,
    /// Empty: no revocation check.
    pub crl: String,
    /// Empty: any peer the CA vouches for.
    pub allowed_peers: String,
    pub min_version: TlsVersion,
    pub max_version: TlsVersion,
    pub crl_end_entity_only: bool,
    pub allow_unknown_revocation_status: bool,
    pub handshake_timeout: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TlsVersion {
    #[serde(rename = "1.2")]
    Tls12,
    #[serde(rename = "1.3")]
    Tls13,
}

impl TlsVersion {
    const fn library(self) -> Version {
        match self {
            Self::Tls12 => Version::Tls12,
            Self::Tls13 => Version::Tls13,
        }
    }
}

impl Default for Tls {
    fn default() -> Self {
        Self {
            enabled: false,
            certificate: String::new(),
            key: String::new(),
            ca: String::new(),
            verify_server_name: true,
            crl: String::new(),
            allowed_peers: String::new(),
            min_version: TlsVersion::Tls12,
            max_version: TlsVersion::Tls13,
            crl_end_entity_only: false,
            allow_unknown_revocation_status: false,
            handshake_timeout: 10,
        }
    }
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
            served: Vec::new(),
            echo: true,
            select_before_execute: false,
            accept_uploads: false,
            upload_folder: std::env::temp_dir().display().to_string(),
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
        let tls = &self.tls;
        let mut builder = TlsConfig::builder(identity, roots)
            .verify_server_name(tls.verify_server_name)
            .versions(tls.min_version.library(), tls.max_version.library())
            .handshake_timeout(Duration::from_secs(tls.handshake_timeout));
        if !tls.allowed_peers.is_empty() {
            let peers = certificates_from_pem_file(&tls.allowed_peers)
                .map_err(|e| format!("TLS allowed peers: {e}"))?;
            builder = builder.allow_peers(peers);
        }
        if !tls.crl.is_empty() {
            let crls = crls_from_pem_file(&tls.crl).map_err(|e| format!("TLS CRL: {e}"))?;
            builder = builder.crls(crls);
            if tls.crl_end_entity_only {
                builder = builder.only_check_end_entity_revocation();
            }
            if tls.allow_unknown_revocation_status {
                builder = builder.allow_unknown_revocation_status();
            }
        }
        builder.build().map_err(|e| format!("TLS: {e}"))
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
pub mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A throwaway CA, leaves `a` and `b` for 127.0.0.1, and a CRL revoking `b`, as PEM files.
    pub fn test_pki(name: &str) -> PathBuf {
        use rcgen::{
            BasicConstraints, CertificateParams, CertificateRevocationListParams, DnType,
            ExtendedKeyUsagePurpose, IsCa, Issuer, KeyIdMethod, KeyPair, KeyUsagePurpose,
            RevokedCertParams, SerialNumber, date_time_ymd,
        };

        let dir =
            std::env::temp_dir().join(format!("iec104-workbench-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |file: &str, text: &str| std::fs::write(dir.join(file), text).unwrap();
        let ca_key = KeyPair::generate().unwrap();
        let mut ca = CertificateParams::new(Vec::<String>::new()).unwrap();
        ca.distinguished_name.push(DnType::CommonName, "test CA");
        ca.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        ca.key_usages = vec![
            KeyUsagePurpose::KeyCertSign,
            KeyUsagePurpose::CrlSign,
            KeyUsagePurpose::DigitalSignature,
        ];
        write("ca.pem", &ca.self_signed(&ca_key).unwrap().pem());
        let issuer = Issuer::new(ca, ca_key);
        for (leaf, serial) in [("a", 1_u64), ("b", 2)] {
            let key = KeyPair::generate().unwrap();
            let mut params = CertificateParams::new(vec!["127.0.0.1".to_string()]).unwrap();
            params.serial_number = Some(SerialNumber::from(serial));
            params.extended_key_usages = vec![
                ExtendedKeyUsagePurpose::ServerAuth,
                ExtendedKeyUsagePurpose::ClientAuth,
            ];
            write(
                &format!("{leaf}.pem"),
                &params.signed_by(&key, &issuer).unwrap().pem(),
            );
            write(&format!("{leaf}.key"), &key.serialize_pem());
        }
        let crl = CertificateRevocationListParams {
            this_update: date_time_ymd(2020, 1, 1),
            next_update: date_time_ymd(2100, 1, 1),
            crl_number: SerialNumber::from(1_u64),
            issuing_distribution_point: None,
            revoked_certs: vec![RevokedCertParams {
                serial_number: SerialNumber::from(2_u64),
                revocation_time: date_time_ymd(2020, 1, 1),
                reason_code: None,
                invalidity_date: None,
            }],
            key_identifier_method: KeyIdMethod::Sha256,
        };
        write("crl.pem", &crl.signed_by(&issuer).unwrap().pem().unwrap());
        dir
    }

    /// TLS settings for 127.0.0.1:`port` with leaf `leaf` of [`test_pki`].
    pub fn tls_settings(pki: &Path, leaf: &str, port: u16) -> Settings {
        let path = |file: &str| pki.join(file).display().to_string();
        let mut settings = Settings {
            port,
            ..Settings::default()
        };
        settings.tls.enabled = true;
        settings.tls.ca = path("ca.pem");
        settings.tls.certificate = path(&format!("{leaf}.pem"));
        settings.tls.key = path(&format!("{leaf}.key"));
        settings
    }

    #[test]
    fn tls_config_takes_the_options() {
        let pki = test_pki("tls-options");
        let mut settings = tls_settings(&pki, "a", 2404);
        let defaults = settings.tls_config();
        settings.tls.crl = pki.join("crl.pem").display().to_string();
        settings.tls.allowed_peers = pki.join("a.pem").display().to_string();
        settings.tls.min_version = TlsVersion::Tls13;
        settings.tls.crl_end_entity_only = true;
        settings.tls.allow_unknown_revocation_status = true;
        settings.tls.handshake_timeout = 3;
        let built = settings.tls_config();
        settings.tls.max_version = TlsVersion::Tls12;
        let inverted = settings.tls_config();
        settings.tls.max_version = TlsVersion::Tls13;
        settings.tls.crl = pki.join("a.pem").display().to_string();
        let not_a_crl = settings.tls_config();
        let _ = std::fs::remove_dir_all(&pki);
        assert!(defaults.is_ok(), "{defaults:?}");
        assert!(built.is_ok(), "{built:?}");
        assert!(inverted.is_err(), "minimum above maximum");
        assert!(not_a_crl.is_err(), "a certificate given as CRL");
    }

    #[test]
    fn defaults_are_valid_and_round_trip() {
        let settings = Settings::default();
        assert!(settings.link().is_ok());
        assert_eq!(settings.sizes().unwrap(), AddressSizes::CONVENTIONAL);
        assert!(settings.tls.verify_server_name, "the library's default");
        let old_profile: Settings = toml::from_str("[tls]\nenabled = true").unwrap();
        assert!(old_profile.tls.verify_server_name);
        assert!(old_profile.tls.crl.is_empty() && old_profile.tls.allowed_peers.is_empty());
        assert_eq!(
            (old_profile.tls.min_version, old_profile.tls.max_version),
            (TlsVersion::Tls12, TlsVersion::Tls13)
        );
        assert!(!old_profile.tls.crl_end_entity_only);
        assert!(!old_profile.tls.allow_unknown_revocation_status);
        assert_eq!(
            old_profile.tls.handshake_timeout, 10,
            "the library's default"
        );
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
