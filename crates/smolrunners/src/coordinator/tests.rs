use smolrunners_core::lifecycle::{Decision, Effect, Lifecycle, RecordingSink, Submitted};
use smolrunners_core::proto::{self, Frame, SCHEMA_VERSION};
use smolrunners_core::store::Store;

use super::{emit_ack, recorded_ack};

fn submitted(recorded: bool) -> Submitted {
    Submitted {
        ack: Frame::Ack {
            schema_version: SCHEMA_VERSION,
            id: "ack-evt-1".to_owned(),
            event_id: "evt-1".to_owned(),
            decision: "accept".to_owned(),
            recorded,
        },
        decision: Decision::Accept,
        state: None,
        effects: Vec::new(),
        replayed: false,
    }
}

#[test]
fn coordinator_rejects_an_unrecorded_ack_before_writing_it() {
    let candidate = submitted(false);
    assert!(recorded_ack(&candidate)
        .unwrap_err()
        .contains("unrecorded ack"));
    let mut wire = Vec::new();
    assert!(emit_ack(&mut wire, &candidate).is_err());
    assert!(wire.is_empty(), "an unrecorded ack must never reach the adapter");
}

#[test]
fn coordinator_emits_the_recorded_ack_only_after_durable_submission() {
    let path = std::env::temp_dir().join(format!(
        "smolrunners-coordinator-ack-{}-{}.sqlite",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&path);
    let store = Store::open(&path).expect("open store");
    let mut lifecycle = Lifecycle::from_store(store).expect("open lifecycle");
    let frame = Frame::Capacity {
        schema_version: SCHEMA_VERSION,
        id: "evt-1".to_owned(),
        assigned_jobs: 1,
        reported_capacity: 1,
    };
    let mut sink = RecordingSink::new();
    let result = lifecycle.submit(&frame, "t=1", &mut sink).expect("submit");
    assert_eq!(sink.performed(), &[] as &[Effect]);
    assert!(recorded_ack(&result).is_ok());
    let mut wire = Vec::new();
    emit_ack(&mut wire, &result).expect("emit ack");
    let decoded = proto::decode(&wire).expect("decode emitted ack");
    assert!(decoded.is_ack());
    assert_eq!(decoded.correlation_id(), "evt-1");
    let _ = std::fs::remove_file(&path);
}
