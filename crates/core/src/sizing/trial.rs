use crate::domain::Shape;

use super::recommend::{Recommendation, Verdict};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialArm {
    Base,
    Candidate,
}

impl TrialArm {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Base => "base",
            Self::Candidate => "candidate",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrialError {
    UnsafeCorpus,
    NoCandidateProfile,
    ZeroRepeats,
}

#[derive(Debug, Clone)]
pub struct PairedTrial {
    pub base_revision: String,
    pub candidate_revision: String,
    pub candidate_shape: Shape,
    pub corpus: String,
    pub safe_to_repeat: bool,
    pub repeats: u32,
}

impl PairedTrial {
    pub fn new(
        base_revision: impl Into<String>,
        candidate_revision: impl Into<String>,
        candidate_shape: Shape,
        corpus: impl Into<String>,
        safe_to_repeat: bool,
        repeats: u32,
    ) -> Result<Self, TrialError> {
        if !safe_to_repeat {
            return Err(TrialError::UnsafeCorpus);
        }
        if repeats == 0 {
            return Err(TrialError::ZeroRepeats);
        }
        Ok(Self {
            base_revision: base_revision.into(),
            candidate_revision: candidate_revision.into(),
            candidate_shape,
            corpus: corpus.into(),
            safe_to_repeat,
            repeats,
        })
    }

    pub fn sequence(&self) -> Vec<TrialArm> {
        let mut arms = Vec::with_capacity(self.repeats as usize * 2);
        for pair in 0..self.repeats {
            if pair % 2 == 0 {
                arms.push(TrialArm::Base);
                arms.push(TrialArm::Candidate);
            } else {
                arms.push(TrialArm::Candidate);
                arms.push(TrialArm::Base);
            }
        }
        arms
    }

    pub fn pairs(&self) -> u32 {
        self.repeats
    }
}

pub fn trial_from_recommendation(
    recommendation: &Recommendation,
    base_revision: impl Into<String>,
    candidate_revision: impl Into<String>,
    corpus: impl Into<String>,
    safe_to_repeat: bool,
    repeats: u32,
) -> Result<PairedTrial, TrialError> {
    let shape = recommendation
        .nearby_profile
        .ok_or(TrialError::NoCandidateProfile)?;
    PairedTrial::new(
        base_revision,
        candidate_revision,
        shape,
        corpus,
        safe_to_repeat,
        repeats,
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeScope {
    NewExecutionsOnly,
}

#[derive(Debug, Clone)]
pub struct ResizeDecision {
    pub from_revision: String,
    pub to_shape: Shape,
    pub scope: ResizeScope,
}

pub fn plan_resize(recommendation: &Recommendation, from_revision: impl Into<String>, active_executions: usize) -> Option<ResizeDecision> {
    if recommendation.verdict != Verdict::Halve && recommendation.verdict != Verdict::Trial {
        return None;
    }
    let to_shape = recommendation.nearby_profile?;
    let _ = active_executions;
    Some(ResizeDecision {
        from_revision: from_revision.into(),
        to_shape,
        scope: ResizeScope::NewExecutionsOnly,
    })
}
