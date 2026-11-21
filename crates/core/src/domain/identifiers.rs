use std::fmt;
use std::num::{IntErrorKind, NonZeroU32};
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::domain::error::{Identifier, IdentifierError};

enum StringRule {
    Opaque,
    JobKey,
}

fn validate_string(
    value: &str,
    field: Identifier,
    max: usize,
    rule: StringRule,
) -> Result<(), IdentifierError> {
    if value.is_empty() {
        return Err(IdentifierError::Empty(field));
    }
    for (index, character) in value.chars().enumerate() {
        if character.is_control() {
            return Err(IdentifierError::ControlCharacter { field, index });
        }
    }
    if value.len() > max {
        return Err(IdentifierError::TooLong {
            field,
            len: value.len(),
            max,
        });
    }
    if value.trim() != value {
        return Err(IdentifierError::InvalidFormat(field));
    }
    if matches!(rule, StringRule::JobKey) {
        let first = value.chars().next().expect("non-empty value");
        if !(first.is_ascii_alphabetic() || first == '_')
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err(IdentifierError::InvalidFormat(field));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ProtocolJobId(String);

impl ProtocolJobId {
    pub const MAX_LEN: usize = 256;

    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_string(&value, Identifier::ProtocolJobId, Self::MAX_LEN, StringRule::Opaque)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for ProtocolJobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ProtocolJobId {
    type Err = IdentifierError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for ProtocolJobId {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct GitHubJobKey(String);

impl GitHubJobKey {
    pub const MAX_LEN: usize = 128;

    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_string(&value, Identifier::GitHubJobKey, Self::MAX_LEN, StringRule::JobKey)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for GitHubJobKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for GitHubJobKey {
    type Err = IdentifierError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for GitHubJobKey {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RunnerName(String);

impl RunnerName {
    pub const MAX_LEN: usize = 255;

    pub fn new(value: impl Into<String>) -> Result<Self, IdentifierError> {
        let value = value.into();
        validate_string(&value, Identifier::RunnerName, Self::MAX_LEN, StringRule::Opaque)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

impl fmt::Display for RunnerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for RunnerName {
    type Err = IdentifierError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

impl<'de> Deserialize<'de> for RunnerName {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

macro_rules! positive_i64_id {
    ($name:ident, $field:expr) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
        #[serde(transparent)]
        pub struct $name(i64);

        impl $name {
            pub fn new(value: i64) -> Result<Self, IdentifierError> {
                if value <= 0 {
                    return Err(IdentifierError::NonPositive($field));
                }
                Ok(Self(value))
            }

            pub const fn get(self) -> i64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl FromStr for $name {
            type Err = IdentifierError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                let parsed = value.parse::<i64>().map_err(|error| match error.kind() {
                    IntErrorKind::PosOverflow | IntErrorKind::NegOverflow => {
                        IdentifierError::Overflow($field)
                    }
                    _ => IdentifierError::InvalidFormat($field),
                })?;
                Self::new(parsed)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
                let value = i64::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

positive_i64_id!(RestJobId, Identifier::RestJobId);
positive_i64_id!(RunnerRequestId, Identifier::RunnerRequestId);
positive_i64_id!(WorkflowRunId, Identifier::WorkflowRunId);
positive_i64_id!(RunnerId, Identifier::RunnerId);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct RunAttempt(NonZeroU32);

impl RunAttempt {
    pub const FIRST: Self = Self(match NonZeroU32::new(1) {
        Some(value) => value,
        None => panic!("1 is nonzero"),
    });

    pub fn new(value: u32) -> Result<Self, IdentifierError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or(IdentifierError::NonPositive(Identifier::RunAttempt))
    }

    pub const fn get(self) -> u32 {
        self.0.get()
    }
}

impl fmt::Display for RunAttempt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RunAttempt {
    type Err = IdentifierError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let parsed = value.parse::<u32>().map_err(|error| match error.kind() {
            IntErrorKind::PosOverflow | IntErrorKind::NegOverflow => {
                IdentifierError::Overflow(Identifier::RunAttempt)
            }
            _ => IdentifierError::InvalidFormat(Identifier::RunAttempt),
        })?;
        Self::new(parsed)
    }
}

impl<'de> Deserialize<'de> for RunAttempt {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u32::deserialize(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_protocol_job_id_accepts_opaque_strings_and_round_trips() {
        for raw in ["00000000-0000-0000-0000-000000000000", "job/42", "ä-task"] {
            let id = ProtocolJobId::from_str(raw).expect("valid protocol job id");
            assert_eq!(id.as_str(), raw);
            assert_eq!(id.to_string(), raw);
        }
    }

    #[test]
    fn domain_protocol_job_id_rejects_empty_control_and_oversized_values() {
        assert_eq!(
            ProtocolJobId::new(""),
            Err(IdentifierError::Empty(Identifier::ProtocolJobId))
        );
        assert_eq!(
            ProtocolJobId::new("bad\u{7}id"),
            Err(IdentifierError::ControlCharacter {
                field: Identifier::ProtocolJobId,
                index: 3
            })
        );
        assert_eq!(
            ProtocolJobId::new("x".repeat(257)),
            Err(IdentifierError::TooLong {
                field: Identifier::ProtocolJobId,
                len: 257,
                max: 256
            })
        );
    }

    #[test]
    fn domain_github_job_key_enforces_workflow_job_id_grammar() {
        for raw in ["build", "_build-2", "A0_z"] {
            assert_eq!(GitHubJobKey::from_str(raw).expect("valid key").as_str(), raw);
        }
        for raw in ["", "1build", "bad.key", "bad key", "-build"] {
            assert!(
                GitHubJobKey::new(raw).is_err(),
                "{raw} must be rejected as a GITHUB_JOB key"
            );
        }
    }

    #[test]
    fn domain_runner_name_accepts_spaces_but_rejects_control_characters() {
        let name = RunnerName::new("build runner 01").expect("valid runner name");
        assert_eq!(name.as_str(), "build runner 01");
        assert!(RunnerName::new("bad\nname").is_err());
        assert_eq!(
            RunnerName::new("x".repeat(256)),
            Err(IdentifierError::TooLong {
                field: Identifier::RunnerName,
                len: 256,
                max: 255
            })
        );
    }

    #[test]
    fn domain_positive_identifiers_reject_zero_negative_and_non_numeric() {
        assert_eq!(RestJobId::new(0), Err(IdentifierError::NonPositive(Identifier::RestJobId)));
        assert!(RunnerRequestId::new(-1).is_err());
        assert_eq!(WorkflowRunId::new(7).expect("positive").get(), 7);
        assert_eq!(RunnerId::from_str("12").expect("positive").get(), 12);
        assert_eq!(
            RestJobId::from_str("nope"),
            Err(IdentifierError::InvalidFormat(Identifier::RestJobId))
        );
        assert_eq!(
            WorkflowRunId::from_str("999999999999999999999999"),
            Err(IdentifierError::Overflow(Identifier::WorkflowRunId))
        );
    }

    #[test]
    fn domain_run_attempt_is_nonzero_and_parses_from_text() {
        assert_eq!(RunAttempt::FIRST.get(), 1);
        assert_eq!(RunAttempt::from_str("3").expect("attempt").get(), 3);
        assert_eq!(
            RunAttempt::new(0),
            Err(IdentifierError::NonPositive(Identifier::RunAttempt))
        );
        assert_eq!(
            RunAttempt::from_str("4294967296"),
            Err(IdentifierError::Overflow(Identifier::RunAttempt))
        );
    }

    #[test]
    fn domain_identifier_serde_rejects_invalid_values_on_deserialize() {
        let id = RestJobId::new(5).expect("positive");
        let json = serde_json::to_string(&id).expect("serialize");
        assert_eq!(json, "5");
        assert_eq!(
            serde_json::from_str::<RestJobId>("0").unwrap_err().to_string(),
            IdentifierError::NonPositive(Identifier::RestJobId).to_string()
        );
        let key = GitHubJobKey::new("build").expect("valid");
        let json = serde_json::to_string(&key).expect("serialize");
        assert_eq!(serde_json::from_str::<GitHubJobKey>(&json).expect("valid"), key);
    }
}
