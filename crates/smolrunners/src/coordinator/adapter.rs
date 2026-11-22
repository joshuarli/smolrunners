use std::collections::HashMap;
use std::io::Read;
use std::process::ChildStdin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use smolrunners_core::proto::{self, Frame};

pub(crate) struct AdapterLink {
    writer: Mutex<Option<ChildStdin>>,
    pending: Mutex<HashMap<String, Sender<Frame>>>,
    sequence: AtomicU64,
}

impl AdapterLink {
    pub(crate) fn new() -> Self {
        Self {
            writer: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            sequence: AtomicU64::new(0),
        }
    }

    pub(crate) fn attach(&self, writer: ChildStdin) {
        if let Ok(mut slot) = self.writer.lock() {
            *slot = Some(writer);
        }
    }

    pub(crate) fn detach(&self) {
        if let Ok(mut slot) = self.writer.lock() {
            *slot = None;
        }
        if let Ok(mut pending) = self.pending.lock() {
            pending.clear();
        }
    }

    fn connected(&self) -> bool {
        self.writer.lock().map(|slot| slot.is_some()).unwrap_or(false)
    }

    pub(crate) fn write(&self, frame: &Frame) -> Result<(), String> {
        let mut slot = self.writer.lock().map_err(|_| "adapter writer lock poisoned".to_owned())?;
        let writer = slot.as_mut().ok_or("adapter is not connected")?;
        proto::write_frame(writer, frame).map_err(|error| error.to_string())
    }

    pub(crate) fn resolve(&self, frame: Frame) {
        let Frame::CommandResult { command_id, .. } = &frame else {
            return;
        };
        let sender = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(command_id));
        if let Some(sender) = sender {
            let _ = sender.send(frame);
        }
    }

    pub(crate) fn command(
        &self,
        build: impl FnOnce(String) -> Frame,
        timeout: Duration,
    ) -> Result<Frame, String> {
        let deadline = Instant::now() + timeout;
        while !self.connected() {
            if Instant::now() >= deadline {
                return Err("adapter is not connected".to_owned());
            }
            thread::sleep(Duration::from_millis(200));
        }
        let id = format!("cmd-{}", self.sequence.fetch_add(1, Ordering::Relaxed) + 1);
        let (sender, receiver) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| "adapter pending lock poisoned".to_owned())?
            .insert(id.clone(), sender);
        let frame = build(id.clone());
        if let Err(error) = self.write(&frame) {
            self.forget(&id);
            return Err(error);
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        let outcome = receiver.recv_timeout(remaining);
        self.forget(&id);
        match outcome {
            Ok(result) => Ok(result),
            Err(RecvTimeoutError::Timeout) => Err(format!("adapter did not answer {id}")),
            Err(RecvTimeoutError::Disconnected) => Err(format!("adapter stopped before answering {id}")),
        }
    }

    fn forget(&self, id: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(id);
        }
    }
}

pub(crate) fn read_frame<R: Read>(reader: &mut R) -> Result<Frame, String> {
    proto::read_frame(reader).map_err(|error| error.to_string())
}
