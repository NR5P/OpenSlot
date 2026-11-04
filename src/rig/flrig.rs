//! FLRig over XML-RPC, as VarAC drives it (`http://host:port`, default localhost:12345,
//! 5 s timeout): `rig.get_vfo`, `rig.set_vfo` (double Hz), `rig.set_ptt` (int 1/0),
//! `rig.get_mode` / `rig.set_mode` (VarAC's UsbMode: "USB", "USB-D", "DATA-U"…), `rig.tune`.

use std::io::{Read, Write};
use std::time::Duration;

use super::extras::interpolate;
use super::{Level, Meter, RadioMode, RigControl, RigError, RigResult, Switch};

pub struct FlrigClient {
    host: String,
    port: u16,
    connected: bool,
    /// FLRig's own name for the data mode (its mode list differs per radio)
    data_mode: Option<String>,
    /// The radio's full power in watts (rig.get_maxpwr; 100 where FLRig doesn't say)
    max_power: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Int(i64),
    Double(f64),
    Str(String),
}

impl Value {
    fn xml(&self) -> String {
        match self {
            Value::Int(i) => format!("<value><int>{i}</int></value>"),
            Value::Double(d) => format!("<value><double>{d:.1}</double></value>"),
            Value::Str(s) => format!("<value><string>{}</string></value>", s.replace('&', "&amp;").replace('<', "&lt;")),
        }
    }
}

pub fn request_xml(method: &str, params: &[Value]) -> String {
    let p: String = params.iter().map(|v| format!("<param>{}</param>", v.xml())).collect();
    format!("<?xml version=\"1.0\"?><methodCall><methodName>{method}</methodName><params>{p}</params></methodCall>")
}

/// The first value of a methodResponse as text (a fault becomes an error).
pub fn response_value(body: &str) -> RigResult<String> {
    if body.contains("<fault>") {
        let msg = between(body, "<string>", "</string>").unwrap_or("fault");
        return Err(RigError::Protocol(format!("FLRig: {msg}")));
    }
    let v = between(body, "<value>", "</value>").ok_or_else(|| RigError::Protocol("FLRig: no value".into()))?;
    // <value><string>x</string></value>, <value><i4>..</i4></value>, or bare <value>x</value>
    let inner = ["string", "i4", "int", "double", "boolean"].iter()
        .find_map(|t| between(v, &format!("<{t}>"), &format!("</{t}>")))
        .unwrap_or(v);
    Ok(inner.trim().replace("&lt;", "<").replace("&amp;", "&"))
}

fn between<'a>(s: &'a str, a: &str, b: &str) -> Option<&'a str> {
    let i = s.find(a)? + a.len();
    let j = s[i..].find(b)? + i;
    Some(&s[i..j])
}

impl FlrigClient {
    pub fn new(host: &str, port: u16) -> Self {
        Self { host: host.to_string(), port, connected: false, data_mode: None, max_power: None }
    }

    fn call(&mut self, method: &str, params: &[Value]) -> RigResult<String> {
        let body = request_xml(method, params);
        let r = (|| -> RigResult<String> {
            let mut s = crate::vara::connect_any(&self.host, self.port, Duration::from_secs(5))?;
            s.set_read_timeout(Some(Duration::from_secs(5)))?;
            s.write_all(format!(
                "POST /RPC2 HTTP/1.1\r\nHost: {}:{}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                self.host, self.port, body.len()
            ).as_bytes())?;
            let mut raw = Vec::new();
            s.read_to_end(&mut raw)?;
            let text = String::from_utf8_lossy(&raw).to_string();
            let body = text.split_once("\r\n\r\n").map(|(_, b)| b).unwrap_or(&text);
            response_value(body)
        })();
        if matches!(r, Err(RigError::Io(_))) {
            self.connected = false;
        }
        r
    }

    /// FLRig's name for USB data on this radio ("USB-D", "DATA-U", "PKTUSB"…).
    fn data_mode_name(&mut self) -> String {
        if let Some(m) = &self.data_mode {
            return m.clone();
        }
        let raw = self.raw_modes();
        let pick = ["USB-D", "DATA-U", "USB-DATA", "PKTUSB", "DIGU", "USB-D1", "D-USB"].iter()
            .find(|m| raw.iter().any(|r| r.eq_ignore_ascii_case(m)))
            .map(|m| m.to_string())
            .unwrap_or_else(|| "USB-D".into());
        self.data_mode = Some(pick.clone());
        pick
    }

    fn number(&mut self, method: &str) -> RigResult<f32> {
        let v = self.call(method, &[])?;
        v.trim().parse::<f32>().map_err(|_| RigError::Protocol(format!("FLRig: bad value {v:?} from {method}")))
    }

    fn full_power(&mut self) -> f32 {
        if self.max_power.is_none() {
            self.max_power = Some(self.number("rig.get_maxpwr").ok().filter(|&w| w >= 1.0).unwrap_or(100.0));
        }
        self.max_power.unwrap_or(100.0)
    }

    fn raw_modes(&mut self) -> Vec<String> {
        // rig.get_modes answers an array of strings
        let Ok(mut s) = crate::vara::connect_any(&self.host, self.port, Duration::from_secs(5)) else { return Vec::new() };
        let req = request_xml("rig.get_modes", &[]);
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.write_all(format!("POST /RPC2 HTTP/1.1\r\nHost: {}\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{req}", self.host, req.len()).as_bytes());
        let mut raw = String::new();
        let _ = s.read_to_string(&mut raw);
        let mut out = Vec::new();
        let mut rest = raw.as_str();
        while let Some(v) = between(rest, "<value>", "</value>") {
            let t = between(v, "<string>", "</string>").unwrap_or(v).trim().to_string();
            if !t.is_empty() && !t.contains('<') {
                out.push(t);
            }
            rest = &rest[rest.find("</value>").map(|i| i + 8).unwrap_or(rest.len())..];
        }
        out
    }
}

impl RigControl for FlrigClient {
    fn connect(&mut self) -> RigResult<()> {
        self.connected = false;
        self.call("rig.get_vfo", &[])?;
        self.connected = true;
        Ok(())
    }

    fn disconnect(&mut self) {
        self.connected = false;
    }

    fn is_connected(&self) -> bool {
        self.connected
    }

    fn get_freq(&mut self) -> RigResult<u64> {
        let v = self.call("rig.get_vfo", &[])?;
        v.parse::<f64>().map(|f| f.round() as u64).map_err(|_| RigError::Protocol(format!("FLRig: bad frequency {v}")))
    }

    fn set_freq(&mut self, hz: u64) -> RigResult<()> {
        self.call("rig.set_vfo", &[Value::Double(hz as f64)]).map(|_| ())
    }

    fn get_mode(&mut self) -> RigResult<RadioMode> {
        let m = self.call("rig.get_mode", &[])?.to_uppercase();
        Ok(match m.as_str() {
            "USB" => RadioMode::Usb,
            "LSB" => RadioMode::Lsb,
            "CW" | "CW-U" | "CW-L" | "CWR" => RadioMode::Cw,
            "AM" => RadioMode::Am,
            "FM" | "FM-N" => RadioMode::Fm,
            "FM-D" | "DATA-FM" | "PKTFM" | "D-FM" => RadioMode::FmData,
            x if x.contains("LSB") || x == "DATA-L" || x == "DIGL" => RadioMode::LsbData,
            _ => RadioMode::UsbData,
        })
    }

    fn set_mode(&mut self, mode: RadioMode) -> RigResult<()> {
        let name = match mode {
            RadioMode::UsbData => self.data_mode_name(),
            RadioMode::Usb => "USB".into(),
            RadioMode::Lsb => "LSB".into(),
            RadioMode::LsbData => "LSB-D".into(),
            RadioMode::Cw => "CW".into(),
            RadioMode::Am => "AM".into(),
            RadioMode::Fm => "FM".into(),
            RadioMode::FmData => "FM-D".into(),
        };
        self.call("rig.set_mode", &[Value::Str(name)]).map(|_| ())
    }

    fn set_ptt(&mut self, on: bool) -> RigResult<()> {
        self.call("rig.set_ptt", &[Value::Int(on as i64)]).map(|_| ())
    }

    fn tune_atu(&mut self) -> RigResult<()> {
        self.call("rig.tune", &[Value::Int(1)]).map(|_| ())
    }

    // FLRig's levels are 0-100 (power in watts); a method this FLRig or radio lacks is a fault
    fn get_level(&mut self, level: Level) -> RigResult<f32> {
        if level == Level::RfPower {
            let full = self.full_power();
            return Ok(self.number("rig.get_power")? / full);
        }
        Ok(self.number(&format!("rig.get_{}", level_method(level)))? / 100.0)
    }

    fn set_level(&mut self, level: Level, value: f32) -> RigResult<()> {
        let n = if level == Level::RfPower { value * self.full_power() } else { value * 100.0 };
        let method = if level == Level::RfPower { "rig.set_power".to_string() } else { format!("rig.set_{}", level_method(level)) };
        self.call(&method, &[Value::Int(n.round() as i64)]).map(|_| ())
    }

    fn get_switch(&mut self, switch: Switch) -> RigResult<bool> {
        Ok(self.number(&format!("rig.get_{}", switch_method(switch)))? != 0.0)
    }

    fn set_switch(&mut self, switch: Switch, on: bool) -> RigResult<()> {
        self.call(&format!("rig.set_{}", switch_method(switch)), &[Value::Int(on as i64)]).map(|_| ())
    }

    fn get_meter(&mut self, meter: Meter) -> RigResult<f32> {
        match meter {
            Meter::Signal => Ok(interpolate(&FLRIG_S, self.number("rig.get_smeter")?)),
            Meter::Power => {
                let full = self.full_power();
                Ok(self.number("rig.get_pwrmeter")? / full)
            }
            Meter::Swr => Ok(interpolate(&FLRIG_SWR, self.number("rig.get_swrmeter")?)),
            Meter::Alc => Err(RigError::Protocol("FLRig has no ALC reading".into())),
        }
    }

    fn name(&self) -> &str {
        "FLRig"
    }
}

/// FLRig's S-meter (0-100: S9 at 50) in dB over S9, and its SWR meter (0-100) as a ratio.
const FLRIG_S: [(f32, f32); 3] = [(0.0, -54.0), (50.0, 0.0), (100.0, 60.0)];
const FLRIG_SWR: [(f32, f32); 5] = [(0.0, 1.0), (25.0, 1.5), (50.0, 2.0), (75.0, 3.0), (100.0, 5.0)];

fn level_method(level: Level) -> &'static str {
    match level {
        Level::RfPower => "power",
        Level::AfGain => "volume",
        Level::RfGain => "rfgain",
        Level::Squelch => "squelch",
        Level::MicGain => "micgain",
    }
}

fn switch_method(switch: Switch) -> &'static str {
    match switch {
        Switch::NoiseBlanker => "noise",
        Switch::NoiseReduction => "noise_reduction",
        Switch::Preamp => "preamp",
        Switch::Attenuator => "attenuator",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_rpc_text() {
        assert_eq!(request_xml("rig.set_vfo", &[Value::Double(14_105_000.0)]),
            "<?xml version=\"1.0\"?><methodCall><methodName>rig.set_vfo</methodName><params><param><value><double>14105000.0</double></value></param></params></methodCall>");
        assert_eq!(response_value("<methodResponse><params><param><value>14105000</value></param></params></methodResponse>").unwrap(), "14105000");
        assert_eq!(response_value("<methodResponse><params><param><value><string>USB-D</string></value></param></params></methodResponse>").unwrap(), "USB-D");
        assert!(response_value("<methodResponse><fault><value><struct><member><name>faultString</name><value><string>no rig</string></value></member></struct></value></fault></methodResponse>").is_err());
    }

    #[test]
    fn talks_to_a_fake_flrig() {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let srv = std::thread::spawn(move || {
            let mut calls = Vec::new();
            for _ in 0..3 {
                let (mut c, _) = l.accept().unwrap();
                let mut buf = vec![0u8; 4096];
                let n = c.read(&mut buf).unwrap();
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                let method = between(&req, "<methodName>", "</methodName>").unwrap().to_string();
                let value = if method == "rig.get_vfo" { "<value>7105000</value>" } else { "<value><i4>0</i4></value>" };
                let body = format!("<?xml version=\"1.0\"?><methodResponse><params><param>{value}</param></params></methodResponse>");
                c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}", body.len()).as_bytes()).unwrap();
                calls.push(req);
            }
            calls
        });
        let mut r = FlrigClient::new("127.0.0.1", port);
        r.connect().unwrap();
        assert!(r.is_connected());
        r.set_freq(14_105_000).unwrap();
        r.set_ptt(true).unwrap();
        let calls = srv.join().unwrap();
        assert!(calls[1].contains("<methodName>rig.set_vfo</methodName>") && calls[1].contains("<double>14105000.0</double>"));
        assert!(calls[2].contains("<methodName>rig.set_ptt</methodName>") && calls[2].contains("<int>1</int>"));
    }
}
