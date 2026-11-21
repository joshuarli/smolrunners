use serde::{Deserialize, Serialize};

use crate::cache::error::{CacheError, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrustTier {
    Contributor,
    Collaborator,
    Maintainer,
}

impl TrustTier {
    pub fn parse(text: &str) -> Result<Self> {
        match text {
            "contributor" => Ok(Self::Contributor),
            "collaborator" => Ok(Self::Collaborator),
            "maintainer" => Ok(Self::Maintainer),
            other => Err(CacheError::InvalidState(format!(
                "unknown trust tier {other:?}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Contributor => "contributor",
            Self::Collaborator => "collaborator",
            Self::Maintainer => "maintainer",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CacheAuthority {
    pub repository: String,
    pub tier: TrustTier,
}

impl CacheAuthority {
    pub fn new(repository: impl Into<String>, tier: TrustTier) -> Self {
        Self {
            repository: repository.into(),
            tier,
        }
    }

    pub fn may_read(&self, target: &Self) -> bool {
        self.repository == target.repository && self.tier >= target.tier
    }

    pub fn may_write(&self, target: &Self) -> bool {
        self.repository == target.repository && self.tier == target.tier
    }

    pub fn require_read(&self, target: &Self) -> Result<()> {
        if self.may_read(target) {
            Ok(())
        } else {
            Err(self.denial(target, "read"))
        }
    }

    pub fn require_write(&self, target: &Self) -> Result<()> {
        if self.may_write(target) {
            Ok(())
        } else {
            Err(self.denial(target, "write"))
        }
    }

    fn denial(&self, target: &Self, operation: &str) -> CacheError {
        let reason = if self.repository != target.repository {
            format!(
                "{operation} crosses repository boundary {} to {}",
                self.repository, target.repository
            )
        } else if self.tier < target.tier {
            format!(
                "{operation} would broaden trust tier from {} to {}",
                self.tier.as_str(),
                target.tier.as_str()
            )
        } else {
            format!("{operation} is not permitted by cache scope")
        };
        CacheError::ScopeDenied {
            repository: target.repository.clone(),
            trust_tier: target.tier.as_str().to_owned(),
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_scope_repository_boundary_is_absolute() {
        let owner = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        let foreign = CacheAuthority::new("other/repo", TrustTier::Contributor);
        assert!(!owner.may_read(&foreign));
        assert!(!owner.may_write(&foreign));
        assert!(matches!(
            owner.require_read(&foreign),
            Err(CacheError::ScopeDenied { reason, .. }) if reason.contains("repository")
        ));
    }

    #[test]
    fn cache_scope_trust_tier_cannot_be_broadened() {
        let low = CacheAuthority::new("acme/widgets", TrustTier::Contributor);
        let high = CacheAuthority::new("acme/widgets", TrustTier::Maintainer);
        assert!(!low.may_read(&high));
        assert!(!low.may_write(&high));
        assert!(matches!(
            low.require_read(&high),
            Err(CacheError::ScopeDenied { reason, .. }) if reason.contains("broaden")
        ));
        assert!(high.may_read(&low));
        assert!(!high.may_write(&low));
    }

    #[test]
    fn cache_scope_write_requires_exact_tier() {
        let a = CacheAuthority::new("acme/widgets", TrustTier::Collaborator);
        let b = CacheAuthority::new("acme/widgets", TrustTier::Collaborator);
        assert!(a.may_write(&b));
        assert!(a.may_read(&b));
    }
}
