use std::collections::HashSet;

use crate::domain::Shape;

use super::grouping::{ComparableRunKey, RunRecord, group_runs};

pub const MIN_COMPARABLE_RUNS: usize = 3;
pub const MIN_SAMPLES: u32 = 10;
pub const MIN_COVERAGE: f64 = 0.95;
pub const BURST_PEAK_TO_MEAN: f64 = 2.5;
pub const BURST_PEAK_TO_ALLOCATED: f64 = 0.75;
pub const DOWNSIZE_PEAK_TO_ALLOCATED: f64 = 0.5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Blocker {
    MissingCoverage,
    ChangedDefinition,
    MixedHardware,
    UnexplainedHostOom,
}

impl Blocker {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingCoverage => "missing-coverage",
            Self::ChangedDefinition => "changed-definition",
            Self::MixedHardware => "mixed-hardware",
            Self::UnexplainedHostOom => "unexplained-host-oom",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    Halve,
    Trial,
    Hold,
    VmShortage,
    Blocked,
    Insufficient,
}

impl Verdict {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Halve => "halve",
            Self::Trial => "trial",
            Self::Hold => "hold",
            Self::VmShortage => "vm-shortage",
            Self::Blocked => "blocked",
            Self::Insufficient => "insufficient",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Confidence {
    Low,
    Medium,
    High,
}

impl Confidence {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Recommendation {
    pub verdict: Verdict,
    pub observation: String,
    pub nearby_profile: Option<Shape>,
    pub confidence: Confidence,
    pub falsifier: String,
    pub blockers: Vec<Blocker>,
    pub comparable_runs: usize,
    pub censored_runs: usize,
}

pub fn half_shape(shape: Shape) -> Shape {
    let vcpu = (shape.vcpu().get() / 2).max(1);
    let memory_gib = (shape.memory_gib() / 2).max(1);
    Shape::new(vcpu, memory_gib).expect("halved shape is valid")
}

fn observation_text(
    runs: usize,
    censored: usize,
    shape: Shape,
    allocated_cpu: f64,
    consumed_cpu: f64,
    peak_cpu: f64,
    allocated_mem: f64,
    consumed_mem: f64,
    peak_mem: f64,
) -> String {
    format!(
        "{runs} comparable runs on {}c/{}g; allocated {allocated_cpu:.1} core-s \
         vs consumed {consumed_cpu:.1} core-s, peak {peak_cpu:.1}/{allocated_cpu:.1} cores; \
         allocated {allocated_mem:.1} gib-s vs consumed {consumed_mem:.1} gib-s, \
         peak {peak_mem:.1}/{allocated_mem:.1} gib; {censored} censored",
        shape.vcpu().get(),
        shape.memory_gib()
    )
}

fn confidence_for(uncensored: usize, censored: usize, bounded: bool) -> Confidence {
    if bounded {
        Confidence::Low
    } else if censored > 0 {
        Confidence::Medium
    } else if uncensored >= 8 {
        Confidence::High
    } else {
        Confidence::Medium
    }
}

pub fn analyze(runs: &[RunRecord]) -> Recommendation {
    let total = runs.len();
    if total == 0 {
        return Recommendation {
            verdict: Verdict::Insufficient,
            observation: "no comparable runs; group by repository, workflow, job, matrix, image, executor, profile revision, hardware class and cache state".to_owned(),
            nearby_profile: None,
            confidence: Confidence::Low,
            falsifier: "collect comparable runs with full coverage before concluding".to_owned(),
            blockers: Vec::new(),
            comparable_runs: 0,
            censored_runs: 0,
        };
    }

    let mut blockers = Vec::new();
    let hardware: HashSet<&str> = runs.iter().map(|r| r.key.hardware_class.as_str()).collect();
    if hardware.len() > 1 {
        blockers.push(Blocker::MixedHardware);
    }
    let definitions: HashSet<&str> = runs.iter().map(|r| r.definition_id.as_str()).collect();
    let cohorts: HashSet<i64> = runs.iter().map(|r| r.cohort).collect();
    let shapes: HashSet<Shape> = runs.iter().map(|r| r.shape).collect();
    if definitions.len() > 1 || cohorts.len() > 1 || shapes.len() > 1 {
        blockers.push(Blocker::ChangedDefinition);
    }
    if runs.iter().any(|r| r.oom.blocks_conclusion()) {
        blockers.push(Blocker::UnexplainedHostOom);
    }
    if runs.iter().any(RunRecord::has_missing_coverage) {
        blockers.push(Blocker::MissingCoverage);
    }

    let censored: Vec<&RunRecord> = runs.iter().filter(|r| r.is_censored()).collect();
    let uncensored: Vec<&RunRecord> = runs.iter().filter(|r| !r.is_censored()).collect();
    let censored_count = censored.len();
    blockers.sort_unstable();

    if !blockers.is_empty() {
        let names = blockers
            .iter()
            .map(|blocker| blocker.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        return Recommendation {
            verdict: Verdict::Blocked,
            observation: format!(
                "{total} comparable runs; {censored_count} censored; downsizing blocked by {names}"
            ),
            nearby_profile: None,
            confidence: Confidence::Low,
            falsifier: format!("resolve {names} and rerun the grouped analysis on the same comparable key"),
            blockers,
            comparable_runs: total,
            censored_runs: censored_count,
        };
    }

    if uncensored.len() < MIN_COMPARABLE_RUNS {
        return Recommendation {
            verdict: Verdict::Insufficient,
            observation: format!(
                "{total} comparable runs of which {} are censored; fewer than {MIN_COMPARABLE_RUNS} uncensored runs"
            , censored_count),
            nearby_profile: None,
            confidence: Confidence::Low,
            falsifier: format!("collect at least {MIN_COMPARABLE_RUNS} comparable uncensored runs with full coverage"),
            blockers: Vec::new(),
            comparable_runs: total,
            censored_runs: censored_count,
        };
    }

    if runs.iter().any(|r| r.oom.is_vm_shortage()) {
        return Recommendation {
            verdict: Verdict::VmShortage,
            observation: format!(
                "{total} comparable runs include a workload-cap OOM at the VM memory limit; \
                 {censored_count} censored"
            ),
            nearby_profile: None,
            confidence: confidence_for(uncensored.len(), censored_count, false),
            falsifier: "a workload-cap OOM at the current cap; a paired trial on a larger shape without OOM would falsify".to_owned(),
            blockers: Vec::new(),
            comparable_runs: total,
            censored_runs: censored_count,
        };
    }

    let shape = uncensored[0].shape;
    let allocated_cpu = f64::from(shape.vcpu().get());
    let allocated_mem = f64::from(shape.memory_gib());
    let consumed_cpu = uncensored.iter().map(|r| r.integrals.consumed_cpu_core_seconds).sum::<f64>()
        / uncensored.len() as f64;
    let consumed_mem = uncensored.iter().map(|r| r.integrals.consumed_memory_gib_seconds).sum::<f64>()
        / uncensored.len() as f64;
    let peak_cpu = uncensored.iter().map(|r| r.peak_cpu_cores).fold(0.0f64, f64::max);
    let peak_mem = uncensored.iter().map(|r| r.peak_memory_gib).fold(0.0f64, f64::max);
    let mean_cpu = uncensored.iter().map(|r| r.mean_cpu_cores()).sum::<f64>() / uncensored.len() as f64;
    let mean_mem = mean_memory_gib_of(uncensored.as_slice());
    let observation = observation_text(
        total,
        censored_count,
        shape,
        allocated_cpu * uncensored[0].wall_seconds,
        consumed_cpu,
        peak_cpu,
        allocated_mem * uncensored[0].wall_seconds,
        consumed_mem,
        peak_mem,
    );

    let cpu_burst = peak_cpu > mean_cpu * BURST_PEAK_TO_MEAN
        || peak_cpu >= allocated_cpu * BURST_PEAK_TO_ALLOCATED;
    let mem_burst = peak_mem > mean_mem * BURST_PEAK_TO_MEAN
        || peak_mem >= allocated_mem * BURST_PEAK_TO_ALLOCATED;
    let low_peak = peak_cpu <= allocated_cpu * DOWNSIZE_PEAK_TO_ALLOCATED
        && peak_mem <= allocated_mem * DOWNSIZE_PEAK_TO_ALLOCATED;
    let candidate = half_shape(shape);

    let (verdict, falsifier, bounded) = if cpu_burst || mem_burst {
        (
            Verdict::Trial,
            "the paired trial's candidate arm repeats without coverage gaps and stays within \
             the candidate shape; a workload-cap OOM or a peak at or above 75% of the current \
             allocation falsifies downsizing".to_owned(),
            true,
        )
    } else if low_peak {
        (
            Verdict::Halve,
            "a paired trial on the candidate shape with a peak at or above 75% of the current \
             allocation, a workload-cap OOM, or any coverage gap falsifies downsizing".to_owned(),
            false,
        )
    } else {
        (
            Verdict::Hold,
            "a paired trial on a smaller shape with full coverage and no OOM would support \
             downsizing".to_owned(),
            true,
        )
    };

    Recommendation {
        verdict,
        observation,
        nearby_profile: if verdict == Verdict::Hold { None } else { Some(candidate) },
        confidence: confidence_for(uncensored.len(), censored_count, bounded),
        falsifier,
        blockers: Vec::new(),
        comparable_runs: total,
        censored_runs: censored_count,
    }
}

fn mean_memory_gib_of(runs: &[&RunRecord]) -> f64 {
    runs.iter().map(|r| r.mean_memory_gib()).sum::<f64>() / runs.len() as f64
}

pub fn recommend_all(runs: impl IntoIterator<Item = RunRecord>) -> Vec<(ComparableRunKey, Recommendation)> {
    let mut recommendations: Vec<_> = group_runs(runs)
        .into_iter()
        .map(|(key, group)| (key, analyze(&group)))
        .collect();
    recommendations.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
    recommendations
}
