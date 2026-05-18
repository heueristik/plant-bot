//! Talks to the FRITZ!Box "AHA-HTTP-Interface" to control DECT smart plugs.
//!
//! This replaces the `fritzapi` crate so the box can be addressed by IP
//! (avoiding the hardcoded `fritz.box` hostname) and so the modern PBKDF2
//! login used by current FRITZ!OS — e.g. on the FRITZ!Box 7530 AX — works.

use serde::Deserialize;
use sha2::Sha256;
use std::env;
use std::thread;
use std::time::Duration;

/// Result type for all FRITZ!Box operations.
pub type Result<T> = std::result::Result<T, FritzError>;

/// Default address of a FRITZ!Box on its own LAN (overridable via `FRITZ_HOST`).
const DEFAULT_HOST: &str = "http://192.168.178.1";

/// Session id returned while not authenticated.
const DEFAULT_SID: &str = "0000000000000000";

/// How many times a transient request failure is attempted before giving up.
const RETRY_ATTEMPTS: usize = 3;

/// Delay between retries of a transient request failure.
const RETRY_DELAY: Duration = Duration::from_secs(3);

/// An error talking to the FRITZ!Box.
#[derive(thiserror::Error, Debug)]
pub enum FritzError {
    /// Missing or invalid configuration — not fixable by retrying.
    #[error("{0}")]
    Config(String),

    /// Network-level failure reaching the FRITZ!Box — worth retrying.
    #[error("cannot reach the FRITZ!Box: {0}")]
    Transport(#[from] reqwest::Error),

    /// The FRITZ!Box answered with an unexpected HTTP status.
    #[error("FRITZ!Box returned HTTP {status} for '{cmd}'")]
    Http {
        cmd: String,
        status: reqwest::StatusCode,
    },

    /// The login was rejected — bad credentials, throttling, or a missing
    /// 'Smart Home' permission. Retrying will not help.
    #[error("{0}")]
    Login(String),

    /// The FRITZ!Box response could not be parsed.
    #[error("unexpected response from the FRITZ!Box: {0}")]
    Parse(#[from] quick_xml::DeError),

    /// The FRITZ!Box accepted the command but the device did not apply it —
    /// typically because it is offline (e.g. not plugged in).
    #[error("device '{ain}' did not apply the command (FRITZ!Box replied '{response}')")]
    DeviceUnavailable { ain: String, response: String },
}

impl FritzError {
    /// Whether retrying the identical request could plausibly succeed.
    ///
    /// Transient failures (lost connection, server-side 5xx) are retryable;
    /// configuration, login and parse failures are permanent.
    fn is_retryable(&self) -> bool {
        match self {
            FritzError::Transport(_) => true,
            FritzError::Http { status, .. } => status.is_server_error(),
            FritzError::Config(_)
            | FritzError::Login(_)
            | FritzError::Parse(_)
            | FritzError::DeviceUnavailable { .. } => false,
        }
    }
}

/// A smart-home device reported by the FRITZ!Box.
#[derive(Debug, Clone)]
pub struct Device {
    /// Actor identification number (AIN), e.g. `"11630 0069103"`.
    pub ain: String,
    pub name: String,
    pub product: String,
    /// Built-in temperature sensor reading in °C.
    pub celsius: f32,
    /// Whether the device is a switchable socket (can be turned on/off).
    pub switchable: bool,
    /// Whether the device is currently online and reachable by the FRITZ!Box.
    pub present: bool,
}

/// A logged-in connection to the FRITZ!Box AHA-HTTP-Interface.
pub struct FritzClient {
    host: String,
    user: String,
    password: String,
    sid: String,
    http: reqwest::blocking::Client,
}

impl FritzClient {
    /// Reads the `FRITZ_*` variables from `.env` and logs in.
    pub fn login() -> Result<Self> {
        dotenvy::dotenv().ok();

        let host = env::var("FRITZ_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
        let user = env::var("FRITZ_USERNAME")
            .map_err(|_| FritzError::Config("FRITZ_USERNAME not found in .env".into()))?;
        let password = env::var("FRITZ_PASSWORD")
            .map_err(|_| FritzError::Config("FRITZ_PASSWORD not found in .env".into()))?;

        // The FRITZ!Box serves the LAN interface with a self-signed certificate;
        // we trust it on purpose because we reach the box directly by IP.
        let http = reqwest::blocking::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()?;

        let mut client = FritzClient {
            host,
            user,
            password,
            sid: DEFAULT_SID.to_string(),
            http,
        };
        client.update_sid()?;
        Ok(client)
    }

    /// Lists all smart-home devices known to the FRITZ!Box.
    pub fn list_devices(&mut self) -> Result<Vec<Device>> {
        let xml = self.aha_request("getdevicelistinfos", None)?;
        let list: DeviceList = quick_xml::de::from_str(&xml)?;
        Ok(list.devices.into_iter().map(Device::from).collect())
    }

    /// Looks up the device named by the required `FRITZ_AIN` env variable.
    ///
    /// The AIN is matched ignoring spaces, so `"11657 0697711"` and
    /// `"116570697711"` are equivalent.
    pub fn configured_device(&mut self) -> Result<Device> {
        let ain = env::var("FRITZ_AIN")
            .map_err(|_| FritzError::Config("FRITZ_AIN not found in .env".into()))?;
        let wanted = ain.replace(' ', "");

        self.list_devices()?
            .into_iter()
            .find(|d| d.ain.replace(' ', "") == wanted)
            .ok_or_else(|| {
                FritzError::Config(format!("no device with AIN '{ain}' found on the FRITZ!Box"))
            })
    }

    /// Switches the socket with the given AIN on.
    pub fn turn_on(&mut self, ain: &str) -> Result<()> {
        self.set_switch(ain, "setswitchon", "1")
    }

    /// Switches the socket with the given AIN off.
    pub fn turn_off(&mut self, ain: &str) -> Result<()> {
        self.set_switch(ain, "setswitchoff", "0")
    }

    /// Sends a switch command and verifies the FRITZ!Box confirms the new
    /// state. An offline device makes the box reply `inval`, which is treated
    /// as a failure rather than a silent success.
    fn set_switch(&mut self, ain: &str, cmd: &str, expected_state: &str) -> Result<()> {
        let state = self.aha_request(cmd, Some(ain))?;
        if state.trim() == expected_state {
            Ok(())
        } else {
            Err(FritzError::DeviceUnavailable {
                ain: ain.to_string(),
                response: state.trim().to_string(),
            })
        }
    }

    /// Sends one AHA command, retrying transient failures a few times.
    fn aha_request(&mut self, cmd: &str, ain: Option<&str>) -> Result<String> {
        for attempt in 1..=RETRY_ATTEMPTS {
            match self.try_aha_request(cmd, ain) {
                Err(err) if err.is_retryable() && attempt < RETRY_ATTEMPTS => {
                    eprintln!(
                        "  '{cmd}' failed (attempt {attempt}/{RETRY_ATTEMPTS}): {err} \
                         — retrying in {}s",
                        RETRY_DELAY.as_secs()
                    );
                    thread::sleep(RETRY_DELAY);
                }
                result => return result,
            }
        }
        unreachable!("the loop returns on the final attempt")
    }

    /// Performs a single AHA command, re-authenticating once on a stale session.
    fn try_aha_request(&mut self, cmd: &str, ain: Option<&str>) -> Result<String> {
        let mut response = self.send_aha(cmd, ain)?;

        // A 403 means the SID timed out (e.g. while waiting between watering
        // cycles); a fresh login and one retry recovers transparently.
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            self.update_sid()?;
            response = self.send_aha(cmd, ain)?;
        }

        let status = response.status();
        if status == reqwest::StatusCode::FORBIDDEN {
            return Err(FritzError::Login(format!(
                "FRITZ!Box rejected '{cmd}' even after re-login — the user likely \
                 lacks the 'Smart Home' permission (System > FRITZ!Box Users)."
            )));
        }
        if !status.is_success() {
            return Err(FritzError::Http {
                cmd: cmd.to_string(),
                status,
            });
        }
        Ok(response.text()?)
    }

    fn send_aha(&self, cmd: &str, ain: Option<&str>) -> Result<reqwest::blocking::Response> {
        let mut request = self
            .http
            .get(format!("{}/webservices/homeautoswitch.lua", self.host))
            .query(&[("switchcmd", cmd), ("sid", &self.sid)]);
        if let Some(ain) = ain {
            request = request.query(&[("ain", ain)]);
        }
        Ok(request.send()?)
    }

    /// Runs the challenge-response login and stores a fresh session id.
    fn update_sid(&mut self) -> Result<()> {
        let challenge = self.session_info(&[("version", "2")])?;
        if challenge.blocked_seconds() > 0 {
            return Err(FritzError::Login(format!(
                "FRITZ!Box is throttling logins after failed attempts; retry in {} s",
                challenge.blocked_seconds()
            )));
        }

        let response = compute_response(&challenge.challenge, &self.password)?;
        let session = self.session_info(&[
            ("version", "2"),
            ("username", &self.user),
            ("response", &response),
        ])?;

        if session.sid == DEFAULT_SID {
            return Err(FritzError::Login(format!(
                "FRITZ!Box login failed for user '{}'. Check FRITZ_USERNAME / \
                 FRITZ_PASSWORD and make sure that user has the 'Smart Home' \
                 permission (System > FRITZ!Box Users in the web UI).",
                self.user
            )));
        }

        self.sid = session.sid;
        Ok(())
    }

    fn session_info(&self, params: &[(&str, &str)]) -> Result<SessionInfo> {
        let xml = self
            .http
            .get(format!("{}/login_sid.lua", self.host))
            .query(params)
            .send()?
            .error_for_status()?
            .text()?;
        Ok(quick_xml::de::from_str(&xml)?)
    }
}

/// Computes the PBKDF2 login response for the FRITZ!Box challenge.
///
/// FRITZ!OS >= 7.24 issues the challenge as `2$<iter1>$<salt1>$<iter2>$<salt2>`,
/// with both salts hex-encoded.
fn compute_response(challenge: &str, password: &str) -> Result<String> {
    let Some(pbkdf2_params) = challenge.strip_prefix("2$") else {
        return Err(FritzError::Login(format!(
            "expected a PBKDF2 login challenge, got '{challenge}'"
        )));
    };

    let parts: Vec<&str> = pbkdf2_params.split('$').collect();
    let [iter1, salt1, iter2, salt2] = parts[..] else {
        return Err(FritzError::Login(format!(
            "malformed PBKDF2 challenge: '{challenge}'"
        )));
    };

    let mut hash1 = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(
        password.as_bytes(),
        &decode_salt(salt1)?,
        parse_iterations(iter1)?,
        &mut hash1,
    );

    let mut hash2 = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(
        &hash1,
        &decode_salt(salt2)?,
        parse_iterations(iter2)?,
        &mut hash2,
    );

    Ok(format!("{salt2}${}", hex::encode(hash2)))
}

fn decode_salt(salt: &str) -> Result<Vec<u8>> {
    hex::decode(salt).map_err(|e| FritzError::Login(format!("invalid challenge salt: {e}")))
}

fn parse_iterations(value: &str) -> Result<u32> {
    value
        .parse()
        .map_err(|e| FritzError::Login(format!("invalid challenge iteration count: {e}")))
}

#[derive(Deserialize)]
struct SessionInfo {
    #[serde(rename = "SID")]
    sid: String,
    #[serde(rename = "Challenge")]
    challenge: String,
    #[serde(rename = "BlockTime", default)]
    block_time: String,
}

impl SessionInfo {
    fn blocked_seconds(&self) -> u32 {
        self.block_time.trim().parse().unwrap_or(0)
    }
}

#[derive(Deserialize)]
struct DeviceList {
    #[serde(rename = "device", default)]
    devices: Vec<DeviceXml>,
}

#[derive(Deserialize)]
struct DeviceXml {
    #[serde(rename = "@identifier")]
    identifier: String,
    #[serde(rename = "@productname")]
    productname: String,
    name: String,
    #[serde(default)]
    present: String,
    /// Present only for devices with a switchable socket, regardless of model.
    switch: Option<serde::de::IgnoredAny>,
    temperature: Option<TemperatureXml>,
}

#[derive(Deserialize)]
struct TemperatureXml {
    celsius: String,
}

impl From<DeviceXml> for Device {
    fn from(xml: DeviceXml) -> Self {
        // The AHA interface reports temperature in units of 0.1 °C.
        let celsius = xml
            .temperature
            .and_then(|t| t.celsius.trim().parse::<f32>().ok())
            .unwrap_or_default()
            * 0.1;
        Device {
            ain: xml.identifier,
            name: xml.name,
            product: xml.productname,
            celsius,
            switchable: xml.switch.is_some(),
            present: xml.present.trim() == "1",
        }
    }
}

#[test]
#[ignore = "requires a live FRITZ!Box and .env credentials"]
fn test_device() {
    let mut client = FritzClient::login().unwrap();
    let devices = client.list_devices().unwrap();
    let ain = devices.first().unwrap().ain.clone();

    for _ in 0..5 {
        client.turn_off(&ain).unwrap();
        thread::sleep(Duration::from_secs(1));

        client.turn_on(&ain).unwrap();
        thread::sleep(Duration::from_secs(1));
    }
}
