use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

use crate::domain::image::ImageTarget;
use crate::domain::shape::Shape;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProfileNameError {
    Empty,
    InvalidFirstCharacter,
    InvalidCharacter(char),
    TooLong { len: usize, max: usize },
}

impl fmt::Display for ProfileNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "profile name must not be empty"),
            Self::InvalidFirstCharacter => {
                write!(f, "profile name must start with a lowercase letter or digit")
            }
            Self::InvalidCharacter(character) => {
                write!(f, "profile name contains invalid character '{character}'")
            }
            Self::TooLong { len, max } => {
                write!(f, "profile name is {len} bytes; maximum is {max}")
            }
        }
    }
}

impl std::error::Error for ProfileNameError {}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProfileName(String);

impl ProfileName {
    pub const MAX_LEN: usize = 64;

    pub fn new(value: impl Into<String>) -> Result<Self, ProfileNameError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ProfileNameError::Empty);
        }
        if value.len() > Self::MAX_LEN {
            return Err(ProfileNameError::TooLong {
                len: value.len(),
                max: Self::MAX_LEN,
            });
        }
        let mut characters = value.chars();
        let first = characters.next().expect("non-empty name");
        if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
            return Err(ProfileNameError::InvalidFirstCharacter);
        }
        if let Some(invalid) = characters.find(|c| {
            !(c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-' || *c == '_')
        }) {
            return Err(ProfileNameError::InvalidCharacter(invalid));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ProfileName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PresetFamily {
    Small,
    Balanced,
    ComputeHeavy,
    MemoryHeavy,
}

impl PresetFamily {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Balanced => "balanced",
            Self::ComputeHeavy => "compute-heavy",
            Self::MemoryHeavy => "memory-heavy",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    name: ProfileName,
    target: ImageTarget,
    shape: Shape,
}

impl Profile {
    pub fn new(name: ProfileName, target: ImageTarget, shape: Shape) -> Self {
        Self {
            name,
            target,
            shape,
        }
    }

    pub fn name(&self) -> &ProfileName {
        &self.name
    }

    pub fn target(&self) -> &ImageTarget {
        &self.target
    }

    pub const fn shape(&self) -> Shape {
        self.shape
    }

    pub fn preset_family(&self) -> Option<PresetFamily> {
        preset_for_shape(self.shape).map(|preset| preset.family())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProfileRevisionId([u8; 32]);

impl ProfileRevisionId {
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }

    pub fn to_hex(&self) -> String {
        let mut text = String::with_capacity(64);
        for byte in self.0 {
            use std::fmt::Write;
            write!(&mut text, "{byte:02x}").expect("writing to a String cannot fail");
        }
        text
    }
}

impl fmt::Display for ProfileRevisionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_hex())
    }
}

impl FromStr for ProfileRevisionId {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        if value.len() != 64 {
            return Err("profile revision id must be 64 hex digits".to_owned());
        }
        let mut bytes = [0u8; 32];
        for (index, chunk) in value.as_bytes().chunks_exact(2).enumerate() {
            let text = std::str::from_utf8(chunk).map_err(|_| "invalid hex".to_owned())?;
            bytes[index] = u8::from_str_radix(text, 16).map_err(|_| "invalid hex".to_owned())?;
        }
        Ok(Self(bytes))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRevision {
    id: ProfileRevisionId,
    number: u32,
    profile: Profile,
}

impl ProfileRevision {
    pub fn new(profile: Profile, number: u32) -> Self {
        let id = Self::compute_id(&profile, number);
        Self {
            id,
            number,
            profile,
        }
    }

    fn compute_id(profile: &Profile, number: u32) -> ProfileRevisionId {
        let mut hasher = Sha256::new();
        hasher.update(b"smolrunners-profile-revision-v1\0");
        hasher.update(number.to_be_bytes());
        hasher.update(profile.name().as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(profile.target().distro().as_label_token().as_bytes());
        hasher.update(b"\0");
        hasher.update(profile.target().release().as_str().as_bytes());
        hasher.update(b"\0");
        hasher.update(profile.target().arch().as_label_token().as_bytes());
        hasher.update(b"\0");
        hasher.update((profile.shape().vcpu().get() as u32).to_be_bytes());
        hasher.update(profile.shape().memory_gib().to_be_bytes());
        let digest = hasher.finalize();
        let mut bytes = [0u8; 32];
        bytes.copy_from_slice(&digest);
        ProfileRevisionId(bytes)
    }

    pub const fn id(&self) -> ProfileRevisionId {
        self.id
    }

    pub const fn number(&self) -> u32 {
        self.number
    }

    pub fn profile(&self) -> &Profile {
        &self.profile
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Preset {
    family: PresetFamily,
    name: &'static str,
    shape: Shape,
}

impl Preset {
    pub const fn new(family: PresetFamily, name: &'static str, vcpu: u16, memory_gib: u32) -> Self {
        let shape = match Shape::new(vcpu, memory_gib) {
            Ok(value) => value,
            Err(_) => panic!("invalid preset shape"),
        };
        Self {
            family,
            name,
            shape,
        }
    }

    pub const fn family(self) -> PresetFamily {
        self.family
    }

    pub const fn shape(self) -> Shape {
        self.shape
    }

    pub fn profile_name(self) -> ProfileName {
        ProfileName::new(self.name).expect("valid preset name")
    }
}

pub static PRESETS: &[Preset] = &[
    Preset::new(PresetFamily::Small, "small-1c2g", 1, 2),
    Preset::new(PresetFamily::Small, "small-2c4g", 2, 4),
    Preset::new(PresetFamily::Balanced, "balanced-4c8g", 4, 8),
    Preset::new(PresetFamily::Balanced, "balanced-8c16g", 8, 16),
    Preset::new(PresetFamily::Balanced, "balanced-16c32g", 16, 32),
    Preset::new(PresetFamily::Balanced, "balanced-32c64g", 32, 64),
    Preset::new(PresetFamily::ComputeHeavy, "compute-heavy-8c8g", 8, 8),
    Preset::new(PresetFamily::ComputeHeavy, "compute-heavy-16c16g", 16, 16),
    Preset::new(PresetFamily::MemoryHeavy, "memory-heavy-2c16g", 2, 16),
    Preset::new(PresetFamily::MemoryHeavy, "memory-heavy-4c32g", 4, 32),
    Preset::new(PresetFamily::MemoryHeavy, "memory-heavy-8c64g", 8, 64),
];

pub fn preset_for_shape(shape: Shape) -> Option<Preset> {
    PRESETS.iter().copied().find(|preset| preset.shape() == shape)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::image::{Arch, Distro, ReleaseToken};

    fn target() -> ImageTarget {
        ImageTarget::new(
            Distro::Debian,
            ReleaseToken::from_parts(13, None),
            Arch::Arm64,
        )
    }

    #[test]
    fn domain_preset_catalog_contains_every_required_shape() {
        let expected = [
            (PresetFamily::Small, 1u16, 2u32),
            (PresetFamily::Small, 2, 4),
            (PresetFamily::Balanced, 4, 8),
            (PresetFamily::Balanced, 8, 16),
            (PresetFamily::Balanced, 16, 32),
            (PresetFamily::Balanced, 32, 64),
            (PresetFamily::ComputeHeavy, 8, 8),
            (PresetFamily::ComputeHeavy, 16, 16),
            (PresetFamily::MemoryHeavy, 2, 16),
            (PresetFamily::MemoryHeavy, 4, 32),
            (PresetFamily::MemoryHeavy, 8, 64),
        ];
        assert_eq!(PRESETS.len(), expected.len());
        for (family, vcpu, gib) in expected {
            let shape = Shape::new(vcpu, gib).expect("preset shape");
            let preset = preset_for_shape(shape).expect("shape is a preset");
            assert_eq!(preset.family(), family);
            assert_eq!(preset.shape(), shape);
        }
    }

    #[test]
    fn domain_custom_shape_is_not_a_preset() {
        let custom = Shape::new(3, 5).expect("valid custom shape");
        assert!(preset_for_shape(custom).is_none());
        let profile = Profile::new(
            ProfileName::new("custom").expect("valid name"),
            target(),
            custom,
        );
        assert_eq!(profile.preset_family(), None);
    }

    #[test]
    fn domain_profile_name_enforces_lowercase_token_grammar() {
        assert_eq!(
            ProfileName::new("balanced-8c16g").expect("valid").as_str(),
            "balanced-8c16g"
        );
        assert!(ProfileName::new("").is_err());
        assert!(ProfileName::new("-start").is_err());
        assert!(ProfileName::new("Upper").is_err());
        assert!(ProfileName::new("bad.name").is_err());
    }

    #[test]
    fn domain_profile_revision_id_is_content_addressed_and_stable() {
        let profile = Profile::new(
            ProfileName::new("balanced-8c16g").expect("valid"),
            target(),
            Shape::new(8, 16).expect("shape"),
        );
        let first = ProfileRevision::new(profile.clone(), 1);
        let same = ProfileRevision::new(profile.clone(), 1);
        let next_number = ProfileRevision::new(profile.clone(), 2);
        let changed_target = Profile::new(
            ProfileName::new("balanced-8c16g").expect("valid"),
            ImageTarget::new(
                Distro::Alpine,
                ReleaseToken::from_parts(3, Some(24)),
                Arch::Arm64,
            ),
            Shape::new(8, 16).expect("shape"),
        );
        assert_eq!(first.id(), same.id());
        assert_ne!(first.id(), next_number.id());
        assert_ne!(first.id(), ProfileRevision::new(changed_target, 1).id());
        assert_eq!(first.id().to_hex().len(), 64);
        assert_eq!(
            ProfileRevisionId::from_str(&first.id().to_hex()).expect("hex id"),
            first.id()
        );
    }

    #[test]
    fn domain_profile_revision_snapshot_stays_immutable() {
        let profile = Profile::new(
            ProfileName::new("small-1c2g").expect("valid"),
            target(),
            Shape::new(1, 2).expect("shape"),
        );
        let revision = ProfileRevision::new(profile, 7);
        assert_eq!(revision.number(), 7);
        assert_eq!(revision.profile().shape(), Shape::new(1, 2).expect("shape"));
        assert_eq!(revision.profile().preset_family(), Some(PresetFamily::Small));
    }
}
