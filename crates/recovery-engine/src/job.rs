//! Async job manager: one scan job per drive, with pause/resume/cancel.
//!
//! The [`JobManager`] owns all live jobs and exposes a typed API that the
//! Tauri IPC layer calls. Each [`JobHandle`] wraps a tokio task and an
//! [`AtomicU8`] state machine for zero-overhead pause/cancel signalling.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use tr_core::{
    FileRecord, JobId, JobState, RecoverReport, RecoverRequest,
    ScanProgress, ScanRequest, SessionId,
};

use crate::pipeline::ScanPipeline;
use crate::recovery::RecoveryWriter;

// ---- State encoding for AtomicU8 ----

const S_QUEUED: u8 = 0;
const S_RUNNING: u8 = 1;
const S_PAUSED: u8 = 2;
const S_FINISHED: u8 = 3;
const S_FAILED: u8 = 4;
const S_CANCELLED: u8 = 5;

fn atomic_to_state(v: u8) -> JobState {
    match v {
        S_QUEUED => JobState::Queued,
        S_RUNNING => JobState::Running,
        S_PAUSED => JobState::Paused,
        S_FINISHED => JobState::Finished,
        S_FAILED => JobState::Failed,
        S_CANCELLED => JobState::Cancelled,
        _ => JobState::Failed,
    }
}

fn state_to_atomic(s: JobState) -> u8 {
    match s {
        JobState::Queued => S_QUEUED,
        JobState::Running => S_RUNNING,
        JobState::Paused => S_PAUSED,
        JobState::Finished => S_FINISHED,
        JobState::Failed => S_FAILED,
        JobState::Cancelled => S_CANCELLED,
    }
}

/// Shared state for a single scan job. Cheap to clone (Arc internals).
#[derive(Debug, Clone)]
pub struct JobControl {
    state: Arc<AtomicU8>,
}

impl JobControl {
    fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(S_QUEUED)),
        }
    }

    /// Current state (lock-free).
    #[must_use]
    pub fn state(&self) -> JobState {
        atomic_to_state(self.state.load(Ordering::Acquire))
    }

    /// Set state (idempotent).
    pub fn set_state(&self, s: JobState) {
        self.state.store(state_to_atomic(s), Ordering::Release);
    }

    /// Returns true if currently paused. The scan loop checks this in its hot path.
    #[must_use]
    pub fn is_paused(&self) -> bool {
        self.state.load(Ordering::Acquire) == S_PAUSED
    }

    /// Returns true if cancelled or failed. The scan loop should exit.
    #[must_use]
    pub fn should_stop(&self) -> bool {
        let s = self.state.load(Ordering::Acquire);
        s == S_CANCELLED || s == S_FAILED
    }

    /// Block (yield) while paused. Returns false if cancelled during pause.
    pub async fn wait_if_paused(&self) -> bool {
        while self.is_paused() {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if self.should_stop() {
                return false;
            }
        }
        !self.should_stop()
    }
}

/// A handle to a running scan job.
#[derive(Debug)]
pub struct JobHandle {
    pub id: JobId,
    pub session_id: SessionId,
    pub request: ScanRequest,
    pub control: JobControl,
    /// Channel to receive progress updates (UI polls this).
    pub progress_rx: mpsc::Receiver<ScanProgress>,
    /// Channel to receive discovered files (streaming to UI).
    pub files_rx: mpsc::Receiver<Vec<FileRecord>>,
    task: Option<JoinHandle<()>>,
}

impl JobHandle {
    /// Current state without blocking.
    #[must_use]
    pub fn state(&self) -> JobState {
        self.control.state()
    }

    /// Pause the scan (the worker will pause at the next checkpoint).
    pub fn pause(&self) {
        let cur = self.control.state.load(Ordering::Acquire);
        if cur == S_RUNNING {
            self.control.set_state(JobState::Paused);
            info!(job = %self.id, "scan paused");
        }
    }

    /// Resume a paused scan.
    pub fn resume(&self) {
        let cur = self.control.state.load(Ordering::Acquire);
        if cur == S_PAUSED {
            self.control.set_state(JobState::Running);
            info!(job = %self.id, "scan resumed");
        }
    }

    /// Cancel the scan.
    pub fn cancel(&self) {
        self.control.set_state(JobState::Cancelled);
        info!(job = %self.id, "scan cancelled");
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
    }
}

/// Manages all active scan jobs. Thread-safe.
#[derive(Debug, Clone)]
pub struct JobManager {
    jobs: Arc<RwLock<HashMap<JobId, Arc<RwLock<JobHandle>>>>>,
}

impl Default for JobManager {
    fn default() -> Self {
        Self::new()
    }
}

impl JobManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            jobs: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Start a new scan job. Returns the job ID immediately; the scan runs
    /// in the background.
    pub fn start_scan(&self, request: ScanRequest) -> JobId {
        let job_id = JobId::new();
        let session_id = request
            .resume_session
            .unwrap_or_else(SessionId::new);
        let control = JobControl::new();

        let (progress_tx, progress_rx) = mpsc::channel(64);
        let (files_tx, files_rx) = mpsc::channel(256);

        let ctrl = control.clone();
        let req = request.clone();
        let jid = job_id;
        let sid = session_id;

        let task = tokio::spawn(async move {
            ctrl.set_state(JobState::Running);
            let result = ScanPipeline::run(
                jid,
                sid,
                &req,
                ctrl.clone(),
                progress_tx,
                files_tx,
            )
            .await;

            match result {
                Ok(()) => {
                    if !ctrl.should_stop() {
                        ctrl.set_state(JobState::Finished);
                    }
                }
                Err(tr_core::Error::Cancelled) => {
                    ctrl.set_state(JobState::Cancelled);
                }
                Err(e) => {
                    warn!(job = %jid, error = %e, "scan failed");
                    ctrl.set_state(JobState::Failed);
                }
            }
        });

        let handle = JobHandle {
            id: job_id,
            session_id,
            request,
            control,
            progress_rx,
            files_rx,
            task: Some(task),
        };

        self.jobs
            .write()
            .insert(job_id, Arc::new(RwLock::new(handle)));

        info!(job = %job_id, session = %session_id, "scan started");
        job_id
    }

    /// Get the current state of a job.
    pub fn job_state(&self, id: JobId) -> Option<JobState> {
        self.jobs
            .read()
            .get(&id)
            .map(|h| h.read().state())
    }

    /// Pause a running job.
    pub fn pause(&self, id: JobId) -> tr_core::Result<()> {
        let jobs = self.jobs.read();
        let h = jobs.get(&id).ok_or_else(|| {
            tr_core::Error::JobNotFound(id.to_string())
        })?;
        h.read().pause();
        Ok(())
    }

    /// Resume a paused job.
    pub fn resume(&self, id: JobId) -> tr_core::Result<()> {
        let jobs = self.jobs.read();
        let h = jobs.get(&id).ok_or_else(|| {
            tr_core::Error::JobNotFound(id.to_string())
        })?;
        h.read().resume();
        Ok(())
    }

    /// Cancel a job.
    pub fn cancel(&self, id: JobId) -> tr_core::Result<()> {
        let jobs = self.jobs.read();
        let h = jobs.get(&id).ok_or_else(|| {
            tr_core::Error::JobNotFound(id.to_string())
        })?;
        h.read().cancel();
        Ok(())
    }

    /// Drain progress updates from a job (non-blocking).
    pub fn drain_progress(&self, id: JobId) -> Vec<ScanProgress> {
        let jobs = self.jobs.read();
        let Some(h) = jobs.get(&id) else {
            return Vec::new();
        };
        let mut handle = h.write();
        let mut out = Vec::new();
        while let Ok(p) = handle.progress_rx.try_recv() {
            out.push(p);
        }
        out
    }

    /// Drain newly discovered files from a job (non-blocking).
    pub fn drain_files(&self, id: JobId) -> Vec<FileRecord> {
        let jobs = self.jobs.read();
        let Some(h) = jobs.get(&id) else {
            return Vec::new();
        };
        let mut handle = h.write();
        let mut out = Vec::new();
        while let Ok(batch) = handle.files_rx.try_recv() {
            out.extend(batch);
        }
        out
    }

    /// Remove a finished/cancelled/failed job from the manager.
    pub fn remove(&self, id: JobId) {
        self.jobs.write().remove(&id);
    }

    /// List all active job IDs.
    #[must_use]
    pub fn active_jobs(&self) -> Vec<(JobId, SessionId, JobState)> {
        self.jobs
            .read()
            .values()
            .map(|h| {
                let h = h.read();
                (h.id, h.session_id, h.state())
            })
            .collect()
    }

    /// Start a recovery (write) operation. This doesn't use the job system
    /// since it's a simpler one-shot task.
    pub async fn recover(
        &self,
        request: RecoverRequest,
        reader: Arc<dyn tr_storage::SectorReader>,
    ) -> tr_core::Result<RecoverReport> {
        RecoveryWriter::recover(request, reader).await
    }
}
