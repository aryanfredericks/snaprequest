//! Flow states kept in memory so a flow can be resumed at a step ("re-run from step N", "next
//! step" in the web UI). They hold captured values, including secret ones, so they are never
//! written to disk or sent to clients, and they expire.

use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use snapreq_test::SessionState;

/// Runs whose states are kept.
const KEEP: usize = 50;
/// How long a state is kept.
const TTL: Duration = Duration::from_secs(3600);

struct Entry {
    run_id: i64,
    collection: String,
    flow: String,
    at: Instant,
    /// State before each step, by step number.
    states: Vec<(usize, SessionState)>,
}

/// The states of recent flow runs.
#[derive(Default)]
pub struct FlowStates {
    entries: Mutex<VecDeque<Entry>>,
}

impl std::fmt::Debug for FlowStates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FlowStates").finish_non_exhaustive()
    }
}

impl FlowStates {
    pub fn insert(
        &self,
        run_id: i64,
        collection: &str,
        flow: &str,
        states: Vec<(usize, SessionState)>,
    ) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.retain(|e| e.at.elapsed() < TTL);
        while entries.len() >= KEEP {
            entries.pop_front();
        }
        entries.push_back(Entry {
            run_id,
            collection: collection.to_owned(),
            flow: flow.to_owned(),
            at: Instant::now(),
            states,
        });
    }

    /// The state before `step` saved by run `run_id` of `flow`.
    pub fn get(
        &self,
        run_id: i64,
        collection: &str,
        flow: &str,
        step: usize,
    ) -> Option<SessionState> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries
            .iter()
            .find(|e| {
                e.run_id == run_id
                    && e.collection == collection
                    && e.flow == flow
                    && e.at.elapsed() < TTL
            })
            .and_then(|e| e.states.iter().find(|(n, _)| *n == step))
            .map(|(_, s)| s.clone())
    }
}
