use std::fmt;
use std::str::FromStr;

use crate::domain::error::{LabelError, ShapeError};
use crate::domain::image::{Arch, Distro, ImageTarget, ReleaseToken};
use crate::domain::profile::{ProfileName, ProfileRevision, ProfileRevisionId};
use crate::domain::shape::Shape;

fn parse_decimal<T: FromStr>(token: &str) -> Option<T> {
    if token.is_empty()
        || !token.bytes().all(|byte| byte.is_ascii_digit())
        || (token.len() > 1 && token.starts_with('0'))
    {
        return None;
    }
    token.parse().ok()
}

fn parse_target(distro_release: &str, arch_token: &str) -> Result<ImageTarget, LabelError> {
    let arch = Arch::from_label_token(arch_token)?;
    for distro in [Distro::Debian, Distro::Alpine] {
        if let Some(release) = distro_release.strip_prefix(distro.as_label_token()) {
            let release = ReleaseToken::new(release)?;
            return Ok(ImageTarget::new(distro, release, arch));
        }
    }
    Err(LabelError::UnknownDistro(distro_release.to_owned()))
}

fn parse_shape(token: &str) -> Result<Shape, LabelError> {
    let (vcpu_token, remainder) = token
        .split_once('c')
        .ok_or_else(|| LabelError::MalformedShape(token.to_owned()))?;
    let memory_token = remainder
        .strip_suffix('g')
        .ok_or_else(|| LabelError::MalformedShape(token.to_owned()))?;
    let vcpu = parse_decimal::<u16>(vcpu_token)
        .ok_or_else(|| LabelError::InvalidVcpu(vcpu_token.to_owned()))?;
    let memory = parse_decimal::<u32>(memory_token)
        .ok_or_else(|| LabelError::InvalidMemory(memory_token.to_owned()))?;
    Shape::new(vcpu, memory).map_err(|error| match error {
        ShapeError::ZeroVcpu | ShapeError::VcpuTooLarge { .. } => {
            LabelError::InvalidVcpu(vcpu_token.to_owned())
        }
        ShapeError::ZeroMemory | ShapeError::MemoryTooLarge { .. } => {
            LabelError::InvalidMemory(memory_token.to_owned())
        }
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ResourceLabel {
    target: ImageTarget,
    shape: Shape,
}

impl ResourceLabel {
    pub const PREFIX: &'static str = "smol-";

    pub fn new(target: ImageTarget, shape: Shape) -> Self {
        Self { target, shape }
    }

    pub fn parse(input: &str) -> Result<Self, LabelError> {
        let rest = input
            .strip_prefix(Self::PREFIX)
            .ok_or(LabelError::MissingPrefix)?;
        let segments: Vec<&str> = rest.split('-').collect();
        if segments.len() != 3 {
            return Err(LabelError::WrongSegmentCount {
                expected: 3,
                found: segments.len(),
            });
        }
        let target = parse_target(segments[0], segments[1])?;
        let shape = parse_shape(segments[2])?;
        Ok(Self::new(target, shape))
    }

    pub fn target(&self) -> &ImageTarget {
        &self.target
    }

    pub const fn shape(&self) -> Shape {
        self.shape
    }
}

impl fmt::Display for ResourceLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "smol-{}{}-{}-{}c{}g",
            self.target.distro().as_label_token(),
            self.target.release().as_str(),
            self.target.arch().as_label_token(),
            self.shape.vcpu().get(),
            self.shape.memory_gib()
        )
    }
}

impl FromStr for ResourceLabel {
    type Err = LabelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AliasLabel {
    name: ProfileName,
}

impl AliasLabel {
    pub const PREFIX: &'static str = "smol-profile-";

    pub fn new(name: ProfileName) -> Self {
        Self { name }
    }

    pub fn parse(input: &str) -> Result<Self, LabelError> {
        let name = input
            .strip_prefix(Self::PREFIX)
            .ok_or(LabelError::MissingAliasPrefix)?;
        let name = ProfileName::new(name).map_err(|error| LabelError::InvalidProfileName(error.to_string()))?;
        Ok(Self::new(name))
    }

    pub fn name(&self) -> &ProfileName {
        &self.name
    }
}

impl fmt::Display for AliasLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", Self::PREFIX, self.name)
    }
}

impl FromStr for AliasLabel {
    type Err = LabelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Label {
    Resource(ResourceLabel),
    Alias(AliasLabel),
}

impl Label {
    pub fn parse(input: &str) -> Result<Self, LabelError> {
        if input.starts_with(AliasLabel::PREFIX) {
            AliasLabel::parse(input).map(Self::Alias)
        } else if input.starts_with(ResourceLabel::PREFIX) {
            ResourceLabel::parse(input).map(Self::Resource)
        } else {
            Err(LabelError::MissingPrefix)
        }
    }
}

impl fmt::Display for Label {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Resource(label) => label.fmt(f),
            Self::Alias(label) => label.fmt(f),
        }
    }
}

impl FromStr for Label {
    type Err = LabelError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProfile {
    alias: AliasLabel,
    revision: ProfileRevision,
}

impl ResolvedProfile {
    pub fn new(alias: AliasLabel, revision: ProfileRevision) -> Result<Self, LabelError> {
        if alias.name() != revision.profile().name() {
            return Err(LabelError::AliasNameMismatch);
        }
        Ok(Self { alias, revision })
    }

    pub fn alias(&self) -> &AliasLabel {
        &self.alias
    }

    pub fn revision(&self) -> &ProfileRevision {
        &self.revision
    }

    pub fn revision_id(&self) -> ProfileRevisionId {
        self.revision.id()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::image::ReleaseToken;
    use crate::domain::profile::{Profile, ProfileName};

    #[test]
    fn domain_resource_label_round_trips_debian_and_alpine() {
        for raw in ["smol-debian13-arm64-4c8g", "smol-alpine324-x64-16c32g"] {
            let label = ResourceLabel::from_str(raw).expect("valid resource label");
            assert_eq!(label.to_string(), raw);
            assert_eq!(
                ResourceLabel::from_str(&label.to_string()).expect("round trip"),
                label
            );
        }
    }

    #[test]
    fn domain_resource_label_decodes_target_and_shape() {
        let label = ResourceLabel::from_str("smol-debian13-arm64-4c8g").expect("valid label");
        assert_eq!(label.target().distro(), Distro::Debian);
        assert_eq!(label.target().release(), &ReleaseToken::from_parts(13, None));
        assert_eq!(label.target().arch(), Arch::Arm64);
        assert_eq!(label.shape(), Shape::new(4, 8).expect("shape"));
    }

    #[test]
    fn domain_resource_label_rejects_malformed_grammar() {
        let cases = [
            ("debian13-arm64-4c8g", LabelError::MissingPrefix),
            (
                "smol-debian13-arm64",
                LabelError::WrongSegmentCount {
                    expected: 3,
                    found: 2,
                },
            ),
            (
                "smol-ubuntu24-arm64-4c8g",
                LabelError::UnknownDistro("ubuntu24".to_owned()),
            ),
            (
                "smol-debian13-riscv64-4c8g",
                LabelError::UnknownArch("riscv64".to_owned()),
            ),
            (
                "smol-debian13x-arm64-4c8g",
                LabelError::InvalidRelease("13x".to_owned()),
            ),
            (
                "smol-debian13-arm64-4x8g",
                LabelError::MalformedShape("4x8g".to_owned()),
            ),
            (
                "smol-debian13-arm64-04c8g",
                LabelError::InvalidVcpu("04".to_owned()),
            ),
            (
                "smol-debian13-arm64-4c08g",
                LabelError::InvalidMemory("08".to_owned()),
            ),
            (
                "smol-debian13-arm64-0c8g",
                LabelError::InvalidVcpu("0".to_owned()),
            ),
        ];
        for (raw, expected) in cases {
            assert_eq!(ResourceLabel::parse(raw), Err(expected), "input {raw}");
        }
    }

    #[test]
    fn domain_label_discriminates_resource_from_alias() {
        assert!(matches!(
            Label::from_str("smol-debian13-arm64-4c8g").expect("resource"),
            Label::Resource(_)
        ));
        assert!(matches!(
            Label::from_str("smol-profile-balanced-8c16g").expect("alias"),
            Label::Alias(_)
        ));
        assert_eq!(
            Label::parse("other"),
            Err(LabelError::MissingPrefix)
        );
    }

    #[test]
    fn domain_alias_label_round_trips_and_validates_name() {
        let alias = AliasLabel::from_str("smol-profile-balanced-8c16g").expect("alias");
        assert_eq!(alias.name().as_str(), "balanced-8c16g");
        assert_eq!(alias.to_string(), "smol-profile-balanced-8c16g");
        assert!(AliasLabel::parse("smol-debian13-arm64-4c8g").is_err());
        assert_eq!(
            AliasLabel::parse("smol-profile-Bad"),
            Err(LabelError::InvalidProfileName(
                crate::domain::profile::ProfileNameError::InvalidFirstCharacter.to_string()
            ))
        );
        assert_eq!(
            AliasLabel::parse("smol-profile-bad.Name"),
            Err(LabelError::InvalidProfileName(
                crate::domain::profile::ProfileNameError::InvalidCharacter('.').to_string()
            ))
        );
    }

    #[test]
    fn domain_alias_resolution_binds_an_immutable_revision() {
        let name = ProfileName::new("balanced-8c16g").expect("name");
        let profile = Profile::new(
            name.clone(),
            ImageTarget::new(
                Distro::Debian,
                ReleaseToken::from_parts(13, None),
                Arch::Arm64,
            ),
            Shape::new(8, 16).expect("shape"),
        );
        let revision = ProfileRevision::new(profile, 3);
        let alias = AliasLabel::new(name);
        let resolved = ResolvedProfile::new(alias, revision.clone()).expect("matching alias");
        assert_eq!(resolved.revision_id(), revision.id());
        assert_eq!(resolved.revision(), &revision);
        assert_eq!(resolved.alias().to_string(), "smol-profile-balanced-8c16g");

        let wrong_alias = AliasLabel::new(ProfileName::new("other").expect("name"));
        assert_eq!(
            ResolvedProfile::new(wrong_alias, revision),
            Err(LabelError::AliasNameMismatch)
        );
    }
}
