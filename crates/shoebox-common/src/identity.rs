//! Stable identity newtypes used across server and client.
//!
//! `UserId`: 16-byte UUID rendered as 32-char lowercase hex.
//! `MachineId`: 16-byte UUID rendered as 32-char lowercase hex.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UserId(pub String);

impl fmt::Display for UserId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for UserId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
            Ok(Self(s.to_string()))
        } else {
            Err(format!("invalid UserId: {s:?} (expected 32 lowercase hex chars)"))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MachineId(pub String);

impl fmt::Display for MachineId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for MachineId {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.len() == 32 && s.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()) {
            Ok(Self(s.to_string()))
        } else {
            Err(format!("invalid MachineId: {s:?} (expected 32 lowercase hex chars)"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_id_parses_lowercase_hex() {
        let s = "0123456789abcdef0123456789abcdef";
        let u: UserId = s.parse().unwrap();
        assert_eq!(u.to_string(), s);
    }

    #[test]
    fn user_id_rejects_uppercase() {
        let s = "0123456789ABCDEF0123456789ABCDEF";
        assert!(s.parse::<UserId>().is_err());
    }

    #[test]
    fn user_id_rejects_wrong_length() {
        assert!("abc".parse::<UserId>().is_err());
        assert!("0123456789abcdef".parse::<UserId>().is_err());
    }

    #[test]
    fn round_trip_via_display() {
        let s = "deadbeefcafebabe0000000011112222";
        let u: UserId = s.parse().unwrap();
        let back: UserId = u.to_string().parse().unwrap();
        assert_eq!(u, back);
    }
}
