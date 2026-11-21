use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Identifier {
    ProtocolJobId,
    RestJobId,
    RunnerRequestId,
    WorkflowRunId,
    RunAttempt,
    GitHubJobKey,
    RunnerId,
    RunnerName,
}

impl Identifier {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ProtocolJobId => "protocol job id",
            Self::RestJobId => "REST job id",
            Self::RunnerRequestId => "runner request id",
            Self::WorkflowRunId => "workflow run id",
            Self::RunAttempt => "run attempt",
            Self::GitHubJobKey => "GITHUB_JOB key",
            Self::RunnerId => "runner id",
            Self::RunnerName => "runner name",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentifierError {
    Empty(Identifier),
    ControlCharacter { field: Identifier, index: usize },
    TooLong { field: Identifier, len: usize, max: usize },
    InvalidFormat(Identifier),
    NonPositive(Identifier),
    Overflow(Identifier),
}

impl fmt::Display for IdentifierError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty(field) => write!(f, "{} must not be empty", field.as_str()),
            Self::ControlCharacter { field, index } => {
                write!(f, "{} has a control character at index {index}", field.as_str())
            }
            Self::TooLong { field, len, max } => {
                write!(f, "{} is {len} bytes; maximum is {max}", field.as_str())
            }
            Self::InvalidFormat(field) => write!(f, "{} has an invalid format", field.as_str()),
            Self::NonPositive(field) => write!(f, "{} must be positive", field.as_str()),
            Self::Overflow(field) => write!(f, "{} is out of range", field.as_str()),
        }
    }
}

impl std::error::Error for IdentifierError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuError {
    ZeroPhysicalCores,
    ZeroThreadsPerCore,
    TooManyThreadsPerCore { max: u16 },
    ZeroVcpus,
    VcpusExceedLogicalCpus { vcpus: u32, logical_cpus: u32 },
}

impl fmt::Display for CpuError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroPhysicalCores => write!(f, "physical core count must be positive"),
            Self::ZeroThreadsPerCore => write!(f, "threads per core must be positive"),
            Self::TooManyThreadsPerCore { max } => {
                write!(f, "threads per core must be at most {max}")
            }
            Self::ZeroVcpus => write!(f, "vCPU count must be positive"),
            Self::VcpusExceedLogicalCpus { vcpus, logical_cpus } => write!(
                f,
                "{vcpus} vCPUs exceed the {logical_cpus} logical CPUs of the host"
            ),
        }
    }
}

impl std::error::Error for CpuError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShapeError {
    ZeroVcpu,
    VcpuTooLarge { max: u16 },
    ZeroMemory,
    MemoryTooLarge { max: u32 },
}

impl fmt::Display for ShapeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroVcpu => write!(f, "vCPU count must be positive"),
            Self::VcpuTooLarge { max } => write!(f, "vCPU count must be at most {max}"),
            Self::ZeroMemory => write!(f, "memory must be positive"),
            Self::MemoryTooLarge { max } => write!(f, "memory must be at most {max} GiB"),
        }
    }
}

impl std::error::Error for ShapeError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LabelError {
    MissingPrefix,
    MissingAliasPrefix,
    WrongSegmentCount { expected: usize, found: usize },
    UnknownDistro(String),
    InvalidRelease(String),
    UnknownArch(String),
    MalformedShape(String),
    InvalidVcpu(String),
    InvalidMemory(String),
    InvalidProfileName(String),
    AliasNameMismatch,
}

impl fmt::Display for LabelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingPrefix => write!(f, "a smolrunners label must start with 'smol-'"),
            Self::MissingAliasPrefix => {
                write!(f, "an alias label must start with 'smol-profile-'")
            }
            Self::WrongSegmentCount { expected, found } => {
                write!(f, "expected {expected} label segments, found {found}")
            }
            Self::UnknownDistro(value) => write!(f, "unknown distribution '{value}'"),
            Self::InvalidRelease(value) => write!(f, "invalid release '{value}'"),
            Self::UnknownArch(value) => write!(f, "unknown architecture '{value}'"),
            Self::MalformedShape(value) => write!(f, "malformed shape segment '{value}'"),
            Self::InvalidVcpu(value) => write!(f, "invalid vCPU count '{value}'"),
            Self::InvalidMemory(value) => write!(f, "invalid memory size '{value}'"),
            Self::InvalidProfileName(value) => write!(f, "invalid profile name '{value}'"),
            Self::AliasNameMismatch => {
                write!(f, "alias name does not match the resolved profile revision")
            }
        }
    }
}

impl std::error::Error for LabelError {}
