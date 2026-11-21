use std::fmt;
use std::str::FromStr;

use crate::domain::error::LabelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Distro {
    Debian,
    Alpine,
}

impl Distro {
    pub const fn as_label_token(self) -> &'static str {
        match self {
            Self::Debian => "debian",
            Self::Alpine => "alpine",
        }
    }

    pub fn from_label_token(token: &str) -> Result<Self, LabelError> {
        match token {
            "debian" => Ok(Self::Debian),
            "alpine" => Ok(Self::Alpine),
            other => Err(LabelError::UnknownDistro(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ReleaseToken(String);

impl ReleaseToken {
    pub const MAX_LEN: usize = 6;

    pub fn new(token: impl Into<String>) -> Result<Self, LabelError> {
        let token = token.into();
        if token.is_empty()
            || !token.bytes().all(|byte| byte.is_ascii_digit())
            || (token.len() > 1 && token.starts_with('0'))
            || token.len() > Self::MAX_LEN
        {
            return Err(LabelError::InvalidRelease(token));
        }
        Ok(Self(token))
    }

    pub fn from_parts(major: u16, minor: Option<u16>) -> Self {
        let token = match minor {
            Some(minor) => format!("{major}{minor}"),
            None => major.to_string(),
        };
        Self::new(token).expect("composed release token is valid")
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ReleaseToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for ReleaseToken {
    type Err = LabelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::new(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arch {
    Arm64,
    X64,
}

impl Arch {
    pub const fn as_label_token(self) -> &'static str {
        match self {
            Self::Arm64 => "arm64",
            Self::X64 => "x64",
        }
    }

    pub fn from_label_token(token: &str) -> Result<Self, LabelError> {
        match token {
            "arm64" => Ok(Self::Arm64),
            "x64" => Ok(Self::X64),
            other => Err(LabelError::UnknownArch(other.to_owned())),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageTarget {
    distro: Distro,
    release: ReleaseToken,
    arch: Arch,
}

impl ImageTarget {
    pub fn new(distro: Distro, release: ReleaseToken, arch: Arch) -> Self {
        Self {
            distro,
            release,
            arch,
        }
    }

    pub const fn distro(&self) -> Distro {
        self.distro
    }

    pub fn release(&self) -> &ReleaseToken {
        &self.release
    }

    pub const fn arch(&self) -> Arch {
        self.arch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_release_token_composes_debian_and_alpine_releases() {
        assert_eq!(ReleaseToken::from_parts(13, None).as_str(), "13");
        assert_eq!(ReleaseToken::from_parts(3, Some(24)).as_str(), "324");
    }

    #[test]
    fn domain_release_token_rejects_non_digits_and_leading_zeroes() {
        assert!(ReleaseToken::new("").is_err());
        assert!(ReleaseToken::new("1a").is_err());
        assert!(ReleaseToken::new("013").is_err());
        assert!(ReleaseToken::new("1234567").is_err());
    }

    #[test]
    fn domain_image_target_round_trips_label_tokens() {
        let target = ImageTarget::new(
            Distro::Debian,
            ReleaseToken::from_parts(13, None),
            Arch::Arm64,
        );
        assert_eq!(Distro::from_label_token(target.distro().as_label_token()), Ok(Distro::Debian));
        assert_eq!(Arch::from_label_token(target.arch().as_label_token()), Ok(Arch::Arm64));
        assert_eq!(target.release().as_str(), "13");
    }
}
