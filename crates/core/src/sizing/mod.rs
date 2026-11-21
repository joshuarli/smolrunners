mod grouping;
pub mod recommend;
mod trial;

pub use grouping::{
    CacheState, Censor, ComparableRunKey, Executor, Integrals, OomClass, Outcome, RunRecord,
    group_runs,
};
pub use recommend::{
    Blocker, Confidence, Recommendation, Verdict, analyze, half_shape, recommend_all,
};
pub use trial::{
    PairedTrial, ResizeDecision, ResizeScope, TrialArm, TrialError, plan_resize,
    trial_from_recommendation,
};

#[cfg(test)]
mod tests;
