use std::fmt;
use std::io::{Read, Write};

use serde::{Deserialize, Serialize};

use crate::domain::{ProtocolJobId, RunnerId, RunnerName, RunnerRequestId};

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    Run {
        schema_version: u32,
        id: String,
        scenario: String,
    },
    Capacity {
        schema_version: u32,
        id: String,
        assigned_jobs: i64,
        reported_capacity: i64,
    },
    JobAvailable {
        schema_version: u32,
        id: String,
        request_id: RunnerRequestId,
        job_id: ProtocolJobId,
        labels: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    JobAssigned {
        schema_version: u32,
        id: String,
        request_id: RunnerRequestId,
        job_id: ProtocolJobId,
        #[serde(skip_serializing_if = "Option::is_none")]
        runner_id: Option<RunnerId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    JobStarted {
        schema_version: u32,
        id: String,
        request_id: RunnerRequestId,
        job_id: ProtocolJobId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runner_name: Option<RunnerName>,
    },
    JobCompleted {
        schema_version: u32,
        id: String,
        request_id: RunnerRequestId,
        job_id: ProtocolJobId,
        #[serde(default)]
        result: String,
    },
    AcquireOutcome {
        schema_version: u32,
        id: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        request_ids: Vec<RunnerRequestId>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        acquired_ids: Vec<RunnerRequestId>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Session {
        schema_version: u32,
        id: String,
        session_event: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        retry_after_ms: Option<i64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    VmDelete {
        schema_version: u32,
        id: String,
        job_id: ProtocolJobId,
    },
    Ack {
        schema_version: u32,
        id: String,
        event_id: String,
        decision: String,
        recorded: bool,
    },
}

impl Frame {
    pub fn schema_version(&self) -> u32 {
        match self {
            Self::Run { schema_version, .. }
            | Self::Capacity { schema_version, .. }
            | Self::JobAvailable { schema_version, .. }
            | Self::JobAssigned { schema_version, .. }
            | Self::JobStarted { schema_version, .. }
            | Self::JobCompleted { schema_version, .. }
            | Self::AcquireOutcome { schema_version, .. }
            | Self::Session { schema_version, .. }
            | Self::VmDelete { schema_version, .. }
            | Self::Ack { schema_version, .. } => *schema_version,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Run { id, .. }
            | Self::Capacity { id, .. }
            | Self::JobAvailable { id, .. }
            | Self::JobAssigned { id, .. }
            | Self::JobStarted { id, .. }
            | Self::JobCompleted { id, .. }
            | Self::AcquireOutcome { id, .. }
            | Self::Session { id, .. }
            | Self::VmDelete { id, .. }
            | Self::Ack { id, .. } => id,
        }
    }

    pub fn correlation_id(&self) -> &str {
        match self {
            Self::Ack { event_id, .. } => event_id,
            _ => self.id(),
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Run { .. } => "run",
            Self::Capacity { .. } => "capacity",
            Self::JobAvailable { .. } => "job_available",
            Self::JobAssigned { .. } => "job_assigned",
            Self::JobStarted { .. } => "job_started",
            Self::JobCompleted { .. } => "job_completed",
            Self::AcquireOutcome { .. } => "acquire_outcome",
            Self::Session { .. } => "session",
            Self::VmDelete { .. } => "vm_delete",
            Self::Ack { .. } => "ack",
        }
    }

    pub fn is_ack(&self) -> bool {
        matches!(self, Self::Ack { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodecError {
    EmptyFrame,
    FrameTooLarge(usize),
    BadSchema(u32),
    TruncatedPrefix,
    TruncatedFrame,
    Encode(String),
    Decode(String),
    Io(String),
}

impl fmt::Display for CodecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyFrame => write!(f, "proto: empty frame"),
            Self::FrameTooLarge(n) => write!(f, "proto: frame exceeds 1 MiB: {n} bytes"),
            Self::BadSchema(v) => write!(f, "proto: unsupported schema_version {v}"),
            Self::TruncatedPrefix => write!(f, "proto: truncated length prefix"),
            Self::TruncatedFrame => write!(f, "proto: truncated frame body"),
            Self::Encode(e) => write!(f, "proto: encode: {e}"),
            Self::Decode(e) => write!(f, "proto: decode: {e}"),
            Self::Io(e) => write!(f, "proto: io: {e}"),
        }
    }
}

impl std::error::Error for CodecError {}

fn validate(frame: &Frame) -> Result<(), CodecError> {
    if frame.schema_version() != SCHEMA_VERSION {
        return Err(CodecError::BadSchema(frame.schema_version()));
    }
    Ok(())
}

pub fn encode(frame: &Frame) -> Result<Vec<u8>, CodecError> {
    validate(frame)?;
    let body = serde_json::to_vec(frame).map_err(|e| CodecError::Encode(e.to_string()))?;
    if body.is_empty() {
        return Err(CodecError::EmptyFrame);
    }
    if body.len() > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge(body.len()));
    }
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

pub fn decode(data: &[u8]) -> Result<Frame, CodecError> {
    if data.len() < 4 {
        return Err(CodecError::TruncatedPrefix);
    }
    let n = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
    if n == 0 {
        return Err(CodecError::EmptyFrame);
    }
    if n > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge(n));
    }
    if data.len() - 4 < n {
        return Err(CodecError::TruncatedFrame);
    }
    let frame: Frame =
        serde_json::from_slice(&data[4..4 + n]).map_err(|e| CodecError::Decode(e.to_string()))?;
    validate(&frame)?;
    Ok(frame)
}

pub fn write_frame<W: Write>(w: &mut W, frame: &Frame) -> Result<(), CodecError> {
    let data = encode(frame)?;
    w.write_all(&data).map_err(|e| CodecError::Io(e.to_string()))
}

pub fn read_frame<R: Read>(r: &mut R) -> Result<Frame, CodecError> {
    let mut prefix = [0u8; 4];
    r.read_exact(&mut prefix)
        .map_err(|e| CodecError::Io(e.to_string()))?;
    let n = u32::from_be_bytes(prefix) as usize;
    if n == 0 {
        return Err(CodecError::EmptyFrame);
    }
    if n > MAX_FRAME_BYTES {
        return Err(CodecError::FrameTooLarge(n));
    }
    let mut body = vec![0u8; n];
    r.read_exact(&mut body)
        .map_err(|e| CodecError::Io(e.to_string()))?;
    let frame: Frame =
        serde_json::from_slice(&body).map_err(|e| CodecError::Decode(e.to_string()))?;
    validate(&frame)?;
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn golden_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../adapter/testdata/frames")
    }

    fn framed(json: &str) -> Vec<u8> {
        let mut bytes = (json.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(json.as_bytes());
        bytes
    }

    #[test]
    fn proto_accepts_frames_without_optional_sdk_fields() {
        for json in [
            r#"{"kind":"job_started","schema_version":1,"id":"e1","request_id":1,"job_id":"j1"}"#,
            r#"{"kind":"job_completed","schema_version":1,"id":"e2","request_id":1,"job_id":"j1"}"#,
        ] {
            let frame = decode(&framed(json)).unwrap_or_else(|error| panic!("{json}: {error}"));
            match frame {
                Frame::JobStarted { runner_name, .. } => assert_eq!(runner_name, None),
                Frame::JobCompleted { result, .. } => assert_eq!(result, ""),
                other => panic!("wrong frame kind: {}", other.kind()),
            }
        }
    }

    #[test]
    fn proto_golden_frames_round_trip() {
        let dir = golden_dir();
        let mut count = 0;
        for entry in std::fs::read_dir(&dir).expect("read golden dir") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("bin") {
                continue;
            }
            let bytes = std::fs::read(&path).expect("read golden frame");
            let frame = decode(&bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            let encoded = encode(&frame).expect("encode golden frame");
            assert_eq!(
                encoded, bytes,
                "re-encoded frame differs from {} ({} bytes)",
                path.display(),
                path.metadata().map(|m| m.len()).unwrap_or(0)
            );
            count += 1;
        }
        assert!(count >= 8, "expected at least 8 golden frames, got {count}");
    }

    #[test]
    fn proto_golden_frame_kinds() {
        for (file, kind) in [
            ("capacity.bin", "capacity"),
            ("job_available.bin", "job_available"),
            ("job_assigned.bin", "job_assigned"),
            ("acquire_outcome.bin", "acquire_outcome"),
            ("session.bin", "session"),
            ("ack.bin", "ack"),
            ("job_completed.bin", "job_completed"),
            ("vm_delete.bin", "vm_delete"),
        ] {
            let bytes = std::fs::read(golden_dir().join(file)).expect("read golden frame");
            let frame = decode(&bytes).expect("decode");
            assert_eq!(frame.kind(), kind, "{file}");
            assert_eq!(frame.schema_version(), SCHEMA_VERSION, "{file}");
        }
    }

    #[test]
    fn proto_rejects_oversize_and_bad_schema() {
        let oversize = Frame::Session {
            schema_version: SCHEMA_VERSION,
            id: "evt-big".to_string(),
            session_event: "x".repeat(MAX_FRAME_BYTES),
            retry_after_ms: None,
            reason: None,
        };
        assert!(matches!(encode(&oversize), Err(CodecError::FrameTooLarge(_))));

        let bad_prefix = [0xFFu8, 0xFF, 0xFF, 0xFF];
        assert_eq!(decode(&bad_prefix), Err(CodecError::FrameTooLarge(u32::MAX as usize)));

        let bad = Frame::Capacity {
            schema_version: 2,
            id: "evt-bad".to_string(),
            assigned_jobs: 1,
            reported_capacity: 1,
        };
        assert!(matches!(encode(&bad), Err(CodecError::BadSchema(2))));
    }

    #[test]
    fn proto_stream_round_trip() {
        let a = Frame::Capacity {
            schema_version: SCHEMA_VERSION,
            id: "evt-stream".to_string(),
            assigned_jobs: 2,
            reported_capacity: 2,
        };
        let b = Frame::Ack {
            schema_version: SCHEMA_VERSION,
            id: "ack-stream".to_string(),
            event_id: "evt-stream".to_string(),
            decision: "accept".to_string(),
            recorded: true,
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &a).expect("write a");
        write_frame(&mut buf, &b).expect("write b");
        let mut cursor = std::io::Cursor::new(buf);
        let got_a = read_frame(&mut cursor).expect("read a");
        let got_b = read_frame(&mut cursor).expect("read b");
        assert_eq!(got_a, a);
        assert_eq!(got_b, b);
        assert_eq!(got_b.correlation_id(), "evt-stream");
    }
}
