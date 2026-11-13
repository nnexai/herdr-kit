use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Deserializer, Serialize};
use std::io::Read;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct WsId(String);

#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct TaskId(String);

impl<'de> Deserialize<'de> for WsId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}
impl<'de> Deserialize<'de> for TaskId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Self::parse(&s).map_err(serde::de::Error::custom)
    }
}

impl WsId {
    pub fn generate() -> Result<Self> {
        let bytes = random::<4>()?;
        let bits = u32::from_be_bytes(bytes) >> 2;
        let alphabet = b"abcdefghijklmnopqrstuvwxyz234567";
        let mut s = String::from("ws-");
        for shift in (0..6).rev() {
            s.push(alphabet[((bits >> (shift * 5)) & 31) as usize] as char);
        }
        Ok(Self(s))
    }
    pub fn parse(s: &str) -> Result<Self> {
        ensure!(
            s.len() == 9
                && s.starts_with("ws-")
                && s.as_bytes()[3..]
                    .iter()
                    .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(b)),
            "invalid workstream id: {s}"
        );
        Ok(Self(s.to_owned()))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TaskId {
    pub fn new(ws: &WsId, n: u32) -> Self {
        assert!(n > 0, "task number must be positive");
        Self(format!("{}-t{n}", ws.as_str()))
    }
    pub fn parse(s: &str) -> Result<Self> {
        ensure!(
            s.len() <= 32 && s.is_ascii() && s.len() >= 12,
            "invalid task id: {s}"
        );
        WsId::parse(&s[..9])?;
        let Some(n) = s[9..].strip_prefix("-t") else {
            bail!("invalid task id: {s}")
        };
        ensure!(
            !n.is_empty()
                && !n.starts_with('0')
                && n.bytes().all(|b| b.is_ascii_digit())
                && n.parse::<u32>().is_ok(),
            "invalid task id: {s}"
        );
        Ok(Self(s.to_owned()))
    }
    pub fn ws(&self) -> WsId {
        WsId(self.0[..9].to_owned())
    }
    pub fn n(&self) -> u32 {
        self.0[11..].parse().expect("validated task id")
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

pub fn random<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0; N];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes)
}

pub fn space_id() -> Result<String> {
    let bytes = random::<16>()?;
    Ok(format!(
        "sp{}",
        bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
    ))
}

pub fn uuid_v4() -> Result<String> {
    let mut b = random::<16>()?;
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[0],
        b[1],
        b[2],
        b[3],
        b[4],
        b[5],
        b[6],
        b[7],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identifiers_validate() {
        let ws = WsId::parse("ws-abc234").unwrap();
        assert_eq!(TaskId::new(&ws, 42).as_str(), "ws-abc234-t42");
        assert_eq!(TaskId::parse("ws-abc234-t42").unwrap().ws(), ws);
        for invalid in ["ws-ABC234", "ws-abc018", "ws-abc23", "ws-abc234x"] {
            assert!(WsId::parse(invalid).is_err());
        }
        for invalid in [
            "ws-abc234-t0",
            "ws-abc234-t01",
            "ws-abc234-t4294967296",
            "ws-abc234-t-1",
            "ws-abc234-t",
        ] {
            assert!(TaskId::parse(invalid).is_err());
        }
        assert!(WsId::parse(WsId::generate().unwrap().as_str()).is_ok());
        let space = space_id().unwrap();
        assert_eq!(space.len(), 34);
        assert!(
            space[2..]
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        let uuid = uuid_v4().unwrap();
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        assert!("89ab".contains(&uuid[19..20]));
    }
}
