//! Talks to the FRITZ!Box "AHA-HTTP-Interface" to control DECT smart plugs.
//!
//! This replaces the `fritzapi` crate so the box can be addressed by IP
//! (avoiding the hardcoded `fritz.box` hostname) and so the modern PBKDF2
//! login used by current FRITZ!OS — e.g. on the FRITZ!Box 7530 AX — works.

use serde::Deserialize;
use sha2::Sha256;
use std::env;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Default address of a FRITZ!Box on its own LAN (overridable via `FRITZ_HOST`).
const DEFAULT_HOST: &str = "http://192.168.178.1";

/// Session id returned while not authenticated.
const DEFAULT_SID: &str = "0000000000000000";

/// A smart-home device reported by the FRITZ!Box.
#[derive(Debug, Clone)]
pub struct Device {
    /// Actor identification number, e.g. `"11630 0069103"`.
    pub ain: String,
    pub name: String,
    pub product: String,
    /// Built-in temperature sensor reading in °C.
    pub celsius: f32,
    /// Whether the device is a switchable socket (can be turned on/off).
    pub switchable: bool,
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
        dotenv::dotenv().ok();

        let host = env::var("FRITZ_HOST").unwrap_or_else(|_| DEFAULT_HOST.to_string());
        let user = env::var("FRITZ_USERNAME").map_err(|_| "FRITZ_USERNAME not found in .env")?;
        let password =
            env::var("FRITZ_PASSWORD").map_err(|_| "FRITZ_PASSWORD not found in .env")?;

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

    /// Switches the socket with the given AIN on.
    pub fn turn_on(&mut self, ain: &str) -> Result<()> {
        self.aha_request("setswitchon", Some(ain)).map(drop)
    }

    /// Switches the socket with the given AIN off.
    pub fn turn_off(&mut self, ain: &str) -> Result<()> {
        self.aha_request("setswitchoff", Some(ain)).map(drop)
    }

    /// Sends one AHA command, re-authenticating once if the session expired.
    fn aha_request(&mut self, cmd: &str, ain: Option<&str>) -> Result<String> {
        let mut response = self.send_aha(cmd, ain)?;

        // A 403 means the SID timed out (e.g. while waiting between watering
        // cycles); a fresh login and one retry recovers transparently.
        if response.status() == reqwest::StatusCode::FORBIDDEN {
            self.update_sid()?;
            response = self.send_aha(cmd, ain)?;
        }

        let status = response.status();
        if !status.is_success() {
            let hint = if status == reqwest::StatusCode::FORBIDDEN {
                " (the logged-in FRITZ!Box user may lack the 'Smart Home' permission)"
            } else {
                ""
            };
            return Err(format!("FRITZ!Box rejected '{cmd}' with HTTP {status}{hint}").into());
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
            return Err(format!(
                "FRITZ!Box is throttling logins after failed attempts; retry in {} s",
                challenge.blocked_seconds()
            )
            .into());
        }

        let response = compute_response(&challenge.challenge, &self.password)?;
        let session = self.session_info(&[
            ("version", "2"),
            ("username", &self.user),
            ("response", &response),
        ])?;

        if session.sid == DEFAULT_SID {
            return Err(format!(
                "FRITZ!Box login failed for user '{}'. Check FRITZ_USERNAME / \
                 FRITZ_PASSWORD and make sure that user has the 'Smart Home' \
                 permission (System > FRITZ!Box Users in the web UI).",
                self.user
            )
            .into());
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

/// Computes the login response for the FRITZ!Box challenge.
///
/// FRITZ!OS >= 7.24 (which includes the 7530 AX) issues a PBKDF2 challenge
/// prefixed with `2$`; older firmware issues a legacy MD5 challenge.
fn compute_response(challenge: &str, password: &str) -> Result<String> {
    let Some(pbkdf2_params) = challenge.strip_prefix("2$") else {
        return Ok(legacy_md5_response(challenge, password));
    };

    // Challenge layout: "2$<iter1>$<salt1>$<iter2>$<salt2>", salts hex-encoded.
    let parts: Vec<&str> = pbkdf2_params.split('$').collect();
    let [iter1, salt1, iter2, salt2] = parts[..] else {
        return Err(format!("malformed PBKDF2 challenge: '{challenge}'").into());
    };

    let mut hash1 = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(
        password.as_bytes(),
        &hex::decode(salt1)?,
        iter1.parse()?,
        &mut hash1,
    );

    let mut hash2 = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(&hash1, &hex::decode(salt2)?, iter2.parse()?, &mut hash2);

    Ok(format!("{salt2}${}", hex::encode(hash2)))
}

/// Legacy MD5 challenge-response for FRITZ!OS older than 7.24.
fn legacy_md5_response(challenge: &str, password: &str) -> String {
    // AVM requires Unicode code points above 255 to be replaced with '.'.
    let cleaned: String = password
        .chars()
        .map(|c| if c as u32 > 255 { '.' } else { c })
        .collect();
    let utf16le: Vec<u8> = format!("{challenge}-{cleaned}")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    format!("{challenge}-{:032x}", md5::compute(utf16le))
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
        }
    }
}

#[test]
#[ignore = "requires a live FRITZ!Box and .env credentials"]
fn test_device() {
    use std::thread;
    use std::time::Duration;

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
