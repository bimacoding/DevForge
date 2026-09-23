//! Parallel / multi-run helpers for the agent runtime.

use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
};

use crate::provider::AgentEvent;

static NEXT_RUN_ID: AtomicU64 = AtomicU64::new(1);

/// Opaque id for one agent run (Ask/Edit/Agent loop).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RunId(pub u64);

impl RunId {
    pub fn new() -> Self {
        Self(NEXT_RUN_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl Default for RunId {
    fn default() -> Self {
        Self::new()
    }
}

/// Agent event tagged with the run that produced it.
#[derive(Debug, Clone)]
pub struct TaggedAgentEvent {
    pub run_id: RunId,
    pub event: AgentEvent,
}

/// How many concurrent runs are allowed (`0` → treat as 1, hard cap 8).
pub fn clamp_max_parallel(n: usize) -> usize {
    n.max(1).min(8)
}

/// Decide whether another run may start given current load and config.
pub fn can_start_another_run(active: usize, max_parallel: usize) -> bool {
    active < clamp_max_parallel(max_parallel)
}

/// Run independent jobs in parallel threads. Each job receives a fresh [`RunId`]
/// and an event sink; events are forwarded on `event_tx` tagged with that id.
///
/// Blocks until every job thread has finished. Drops `event_tx` after spawn so
/// the channel closes once workers exit (callers should drain `rx` on another
/// thread or after this returns if they kept a clone of `tx` — prefer draining
/// concurrently).
pub fn run_parallel_jobs<F>(jobs: Vec<F>, event_tx: mpsc::Sender<TaggedAgentEvent>)
where
    F: FnOnce(RunId, &mut dyn FnMut(AgentEvent)) + Send + 'static,
{
    let mut handles = Vec::with_capacity(jobs.len());
    for job in jobs {
        let tx = event_tx.clone();
        let run_id = RunId::new();
        handles.push(thread::spawn(move || {
            let mut emit = |event: AgentEvent| {
                let _ = tx.send(TaggedAgentEvent { run_id, event });
            };
            job(run_id, &mut emit);
        }));
    }
    drop(event_tx);
    for h in handles {
        let _ = h.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn clamp_max_parallel_bounds() {
        assert_eq!(clamp_max_parallel(0), 1);
        assert_eq!(clamp_max_parallel(3), 3);
        assert_eq!(clamp_max_parallel(99), 8);
    }

    #[test]
    fn can_start_respects_cap() {
        assert!(can_start_another_run(0, 2));
        assert!(can_start_another_run(1, 2));
        assert!(!can_start_another_run(2, 2));
        assert!(!can_start_another_run(1, 1));
    }

    #[test]
    fn parallel_jobs_tag_events_by_run() {
        type Job = Box<dyn FnOnce(RunId, &mut dyn FnMut(AgentEvent)) + Send>;
        let (tx, rx) = mpsc::channel();
        let jobs: Vec<Job> = vec![
            Box::new(|_id, emit| {
                emit(AgentEvent::TextDelta("one".into()));
                emit(AgentEvent::Done);
            }),
            Box::new(|_id, emit| {
                emit(AgentEvent::TextDelta("two".into()));
                emit(AgentEvent::Done);
            }),
        ];

        let worker = thread::spawn(move || run_parallel_jobs(jobs, tx));
        let events: Vec<_> = rx.iter().collect();
        worker.join().unwrap();

        assert_eq!(events.len(), 4);
        let mut ids: Vec<_> = events.iter().map(|e| e.run_id.0).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), 2);

        let texts: Vec<_> = events
            .iter()
            .filter_map(|e| match &e.event {
                AgentEvent::TextDelta(t) => Some(t.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t == "one"));
        assert!(texts.iter().any(|t| t == "two"));
        let _ = Arc::new(());
    }
}
