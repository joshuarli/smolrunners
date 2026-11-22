use crate::domain::Shape;

use super::grouping::{
    CacheState, Censor, ComparableRunKey, Executor, Integrals, OomClass, Outcome, RunRecord,
    group_runs,
};
use super::recommend::{Blocker, Confidence, Verdict, analyze, recommend_all};
use super::trial::{
    PairedTrial, ResizeScope, TrialArm, TrialError, plan_resize, trial_from_recommendation,
};

fn key() -> ComparableRunKey {
    ComparableRunKey::new(
        "o/r",
        "build",
        "test",
        "arm64",
        "debian13@sha256:abc",
        Executor::Fixture,
        "rev-1",
        "m5-arm64-nested",
        CacheState::Warm,
    )
}

fn shape() -> Shape {
    Shape::new(8, 16).expect("shape")
}

fn integrals(vcpu: u16, gib: u32, wall: f64, mean_cpu: f64, mean_mem: f64) -> Integrals {
    Integrals {
        allocated_cpu_core_seconds: f64::from(vcpu) * wall,
        consumed_cpu_core_seconds: mean_cpu * wall,
        allocated_memory_gib_seconds: f64::from(gib) * wall,
        consumed_memory_gib_seconds: mean_mem * wall,
    }
}

fn run() -> RunRecord {
    RunRecord {
        key: key(),
        outcome: Outcome::Success,
        oom: OomClass::None,
        censor: Censor::None,
        integrals: integrals(8, 16, 100.0, 2.0, 4.0),
        peak_cpu_cores: 8.0,
        peak_memory_gib: 4.0,
        shape: shape(),
        wall_seconds: 100.0,
        sample_count: 100,
        coverage_fraction: 1.0,
        gap_count: 0,
        definition_id: "job-def-v1".to_owned(),
        cohort: 1,
    }
}

fn low_run() -> RunRecord {
    let mut record = run();
    record.integrals = integrals(8, 16, 100.0, 1.0, 2.0);
    record.peak_cpu_cores = 2.0;
    record.peak_memory_gib = 2.0;
    record
}

#[test]
fn sizing_groups_by_the_nine_comparable_dimensions_and_keeps_failures_and_ooms() {
    let success = run();
    let mut failed = run();
    failed.outcome = Outcome::Failed;
    let mut oom = run();
    oom.outcome = Outcome::Oom;
    oom.oom = OomClass::ServiceCap;
    oom.censor = Censor::Oom;
    let mut cold = run();
    cold.key.cache_state = CacheState::Cold;

    let groups = group_runs([success.clone(), failed, oom, cold.clone()]);
    assert_eq!(groups.len(), 2);
    let warm = groups.get(&key()).expect("warm group");
    assert_eq!(warm.len(), 3);
    assert!(warm.iter().any(|r| r.outcome == Outcome::Failed));
    assert!(warm.iter().any(|r| r.outcome == Outcome::Oom));
    let cold_key = {
        let mut value = key();
        value.cache_state = CacheState::Cold;
        value
    };
    assert_eq!(groups.get(&cold_key).expect("cold group").len(), 1);

    let recommendations = recommend_all([success, cold]);
    assert_eq!(recommendations.len(), 2);
    assert_eq!(recommendations[0].0.cache_state, CacheState::Cold);
    assert_eq!(recommendations[1].0.cache_state, CacheState::Warm);
}

#[test]
fn sizing_eight_core_burst_at_a_quarter_mean_is_a_trial_not_a_halve() {
    let runs: Vec<RunRecord> = (0..4).map(|_| run()).collect();
    let report = analyze(&runs);
    assert_eq!(report.verdict, Verdict::Trial);
    assert_ne!(report.verdict, Verdict::Halve);
    assert!(report.blockers.is_empty());
    assert_eq!(report.nearby_profile, Some(Shape::new(4, 8).expect("candidate")));
    assert_eq!(report.comparable_runs, 4);
    assert!(report.observation.contains("allocated 800.0 core-s"));
    assert!(report.observation.contains("consumed 200.0 core-s"));
    assert!(!report.falsifier.is_empty());
}

#[test]
fn sizing_service_cap_oom_is_not_a_vm_shortage() {
    let mut service_oom = low_run();
    service_oom.outcome = Outcome::Oom;
    service_oom.oom = OomClass::ServiceCap;
    service_oom.censor = Censor::Oom;
    let runs = vec![low_run(), low_run(), low_run(), service_oom];
    let report = analyze(&runs);
    assert!(!OomClass::ServiceCap.is_vm_shortage());
    assert_ne!(report.verdict, Verdict::VmShortage);
    assert_eq!(report.verdict, Verdict::Halve);
    assert_eq!(report.censored_runs, 1);
    assert!(report.blockers.is_empty());
}

#[test]
fn sizing_workload_cap_oom_is_a_vm_shortage_and_blocks_downsizing() {
    let mut oom = low_run();
    oom.outcome = Outcome::Oom;
    oom.oom = OomClass::WorkloadCap;
    oom.censor = Censor::Oom;
    let runs = vec![low_run(), low_run(), low_run(), oom];
    let report = analyze(&runs);
    assert_eq!(report.verdict, Verdict::VmShortage);
    assert_eq!(report.nearby_profile, None);
}

#[test]
fn sizing_blocks_on_missing_coverage_changed_definition_mixed_hardware_and_host_oom() {
    let mut missing = run();
    missing.gap_count = 1;
    let mut changed = run();
    changed.definition_id = "job-def-v2".to_owned();
    let mut mixed = run();
    mixed.key.hardware_class = "native-x64".to_owned();
    let mut host_oom = run();
    host_oom.oom = OomClass::HostOom { explained: false };

    for (record, expected) in [
        (missing, Blocker::MissingCoverage),
        (changed, Blocker::ChangedDefinition),
        (mixed, Blocker::MixedHardware),
        (host_oom, Blocker::UnexplainedHostOom),
    ] {
        let runs = vec![low_run(), low_run(), low_run(), record];
        let report = analyze(&runs);
        assert_eq!(report.verdict, Verdict::Blocked);
        assert!(report.blockers.contains(&expected));
        assert_eq!(report.confidence, Confidence::Low);
        assert_eq!(report.nearby_profile, None);
    }

    let mut changed_cohort = run();
    changed_cohort.cohort = 2;
    let report = analyze(&[low_run(), low_run(), low_run(), changed_cohort]);
    assert!(report.blockers.contains(&Blocker::ChangedDefinition));
}

#[test]
fn sizing_explained_host_oom_does_not_block() {
    let mut explained = low_run();
    explained.oom = OomClass::HostOom { explained: true };
    explained.censor = Censor::Oom;
    let runs = vec![low_run(), low_run(), low_run(), explained];
    let report = analyze(&runs);
    assert!(!report.blockers.contains(&Blocker::UnexplainedHostOom));
    assert_ne!(report.verdict, Verdict::Blocked);
}

#[test]
fn sizing_halves_low_flat_runs_with_high_confidence_and_labels_integrals() {
    let runs: Vec<RunRecord> = (0..8).map(|_| low_run()).collect();
    let report = analyze(&runs);
    assert_eq!(report.verdict, Verdict::Halve);
    assert_eq!(report.confidence, Confidence::High);
    assert_eq!(report.nearby_profile, Some(Shape::new(4, 8).expect("candidate")));
    assert_eq!(report.comparable_runs, 8);
    assert_eq!(report.censored_runs, 0);
    assert_eq!(runs[0].integrals.cpu_utilization(), 0.125);
    assert_eq!(runs[0].integrals.memory_utilization(), 0.125);
    assert!(report.observation.contains("allocated 1600.0 gib-s"));
    assert!(report.observation.contains("consumed 200.0 gib-s"));
}

#[test]
fn sizing_holds_when_utilization_is_neither_low_nor_bursty() {
    let mut steady = low_run();
    steady.integrals = integrals(8, 16, 100.0, 4.8, 9.6);
    steady.peak_cpu_cores = 5.0;
    steady.peak_memory_gib = 10.0;
    let runs = vec![steady.clone(), steady.clone(), steady];
    let report = analyze(&runs);
    assert_eq!(report.verdict, Verdict::Hold);
    assert_eq!(report.nearby_profile, None);
}

#[test]
fn sizing_insufficient_with_few_or_all_censored_runs() {
    assert_eq!(analyze(&[low_run(), low_run()]).verdict, Verdict::Insufficient);
    assert_eq!(analyze(&[]).verdict, Verdict::Insufficient);
    let censored: Vec<RunRecord> = (0..4)
        .map(|_| {
            let mut record = low_run();
            record.censor = Censor::HardLimit;
            record
        })
        .collect();
    let report = analyze(&censored);
    assert_eq!(report.verdict, Verdict::Insufficient);
    assert_eq!(report.censored_runs, 4);
}

#[test]
fn sizing_paired_trial_is_interleaved_repeatable_and_safe_only() {
    assert_eq!(
        PairedTrial::new("r1", "r2", shape(), "corpus", false, 3).unwrap_err(),
        TrialError::UnsafeCorpus
    );
    assert_eq!(
        PairedTrial::new("r1", "r2", shape(), "corpus", true, 0).unwrap_err(),
        TrialError::ZeroRepeats
    );
    let trial = PairedTrial::new("r1", "r2", shape(), "corpus", true, 3).expect("trial");
    let sequence = trial.sequence();
    assert_eq!(sequence.len(), 6);
    assert_eq!(sequence.iter().filter(|arm| **arm == TrialArm::Base).count(), 3);
    assert_eq!(sequence.iter().filter(|arm| **arm == TrialArm::Candidate).count(), 3);
    assert_eq!(
        sequence,
        vec![
            TrialArm::Base,
            TrialArm::Candidate,
            TrialArm::Candidate,
            TrialArm::Base,
            TrialArm::Base,
            TrialArm::Candidate,
        ]
    );
    assert_eq!(trial.sequence(), trial.sequence());
    assert_eq!(trial.pairs(), 3);
}

#[test]
fn sizing_trial_requires_a_candidate_profile_from_the_recommendation() {
    let runs: Vec<RunRecord> = (0..4).map(|_| run()).collect();
    let report = analyze(&runs);
    let trial = trial_from_recommendation(&report, "r1", "r2", "corpus", true, 2).expect("trial");
    assert_eq!(trial.candidate_shape, Shape::new(4, 8).expect("candidate"));

    let mut blocked = low_run();
    blocked.gap_count = 1;
    let report = analyze(&[low_run(), low_run(), low_run(), blocked]);
    assert_eq!(
        trial_from_recommendation(&report, "r1", "r2", "corpus", true, 2).unwrap_err(),
        TrialError::NoCandidateProfile
    );
}

#[test]
fn sizing_never_resizes_active_jobs() {
    let runs: Vec<RunRecord> = (0..8).map(|_| low_run()).collect();
    let report = analyze(&runs);
    let decision = plan_resize(&report, "rev-1", 12).expect("decision");
    assert_eq!(decision.scope, ResizeScope::NewExecutionsOnly);
    assert_eq!(decision.to_shape, Shape::new(4, 8).expect("candidate"));
    assert_eq!(decision.from_revision, "rev-1");

    let mut blocked = low_run();
    blocked.gap_count = 1;
    let report = analyze(&[low_run(), low_run(), low_run(), blocked]);
    assert!(plan_resize(&report, "rev-1", 0).is_none());
}
