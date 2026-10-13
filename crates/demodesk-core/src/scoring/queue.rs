//! Session-only FIFO analysis jobs. The Engine owns the single worker.
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Status {
    Queued,
    Running,
    Done,
    Error,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub demo_id: String,
    pub sequence: u64,
    pub revision: u64,
    pub status: Status,
    pub step: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Phase>,
    pub error: Option<String>,
    pub created_at: String,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub force: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    WaitingForCompute,
    Preparing,
    Analyzing,
    Saving,
}
#[derive(Default)]
pub struct Queue {
    pub jobs: Vec<Job>,
    pub worker_running: bool,
}
impl Queue {
    pub fn enqueue(&mut self, demo_id: &str, force: bool) -> Job {
        if let Some(job) = self
            .jobs
            .iter()
            .find(|j| j.demo_id == demo_id && matches!(j.status, Status::Queued | Status::Running))
        {
            return job.clone();
        }
        let sequence = self.jobs.iter().map(|j| j.sequence).max().unwrap_or(0) + 1;
        let job = Job {
            id: format!("analysis-{sequence}"),
            demo_id: demo_id.into(),
            sequence,
            revision: 0,
            status: Status::Queued,
            step: None,
            phase: None,
            error: None,
            created_at: crate::store::now(),
            started_at: None,
            finished_at: None,
            force,
        };
        self.jobs.push(job.clone());
        job
    }
    pub fn claim(&mut self) -> Option<Job> {
        let job = self
            .jobs
            .iter_mut()
            .filter(|j| j.status == Status::Queued)
            .min_by_key(|j| j.sequence)?;
        job.status = Status::Running;
        job.step = Some(1);
        job.phase = Some(Phase::Preparing);
        job.started_at = Some(crate::store::now());
        job.revision += 1;
        Some(job.clone())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fifo_deduplicates_active_jobs_and_allows_retry() {
        let mut queue = Queue::default();
        let a = queue.enqueue("a", true);
        let b = queue.enqueue("b", true);
        assert_eq!(queue.enqueue("a", true).id, a.id);
        assert_eq!(queue.claim().unwrap().id, a.id);
        assert_eq!(queue.enqueue("a", true).id, a.id);
        queue.jobs[0].status = Status::Error;
        let retry = queue.enqueue("a", true);
        assert_ne!(retry.id, a.id);
        assert_eq!(queue.claim().unwrap().id, b.id);
        assert_eq!(queue.claim().unwrap().id, retry.id);
        assert!(queue.claim().is_none());
    }
    #[test]
    fn older_analysis_jobs_load_without_phase_and_new_phases_are_typed() {
        let mut queue = Queue::default();
        let job = queue.enqueue("fixture", false);
        let mut json = serde_json::to_value(job).unwrap();
        json.as_object_mut().unwrap().remove("phase");
        let old: Job = serde_json::from_value(json.clone()).unwrap();
        assert!(old.phase.is_none());
        json["phase"] = serde_json::json!("waiting-for-compute");
        assert_eq!(
            serde_json::from_value::<Job>(json.clone()).unwrap().phase,
            Some(Phase::WaitingForCompute)
        );
        json["phase"] = serde_json::json!("invented-phase");
        assert!(serde_json::from_value::<Job>(json).is_err());
    }
}
