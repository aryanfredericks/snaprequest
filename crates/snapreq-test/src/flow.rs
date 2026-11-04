//! Running a flow: its steps in order, with delays and retries, stopping at the first
//! failing step.

use std::time::Duration;

use snapreq_collection::FlowItem;
use snapreq_core::Request;

use crate::session::{RequestRun, RunOutcome, SendSpec, Session, SessionState, Target};

/// Which steps of a flow to run (1-based, inclusive) and the state to start from.
#[derive(Debug, Clone, Default)]
pub struct FlowSteps {
    /// First step to run; `0` or `1` is the first step.
    pub from: usize,
    /// Last step to run; `None` runs to the end.
    pub to: Option<usize>,
    /// Captured values (and hidden secrets) the steps before `from` left behind.
    pub start: Option<SessionState>,
}

/// One executed step.
#[derive(Debug)]
pub struct StepRun {
    /// 1-based.
    pub number: usize,
    pub title: String,
    /// How often it was sent (more than 1 with `retry`).
    pub attempts: u32,
    /// The last attempt.
    pub run: RequestRun,
}

/// The result of a flow.
#[derive(Debug)]
pub struct FlowRun {
    pub key: String,
    pub name: String,
    /// Executed steps; a failing step is the last one.
    pub steps: Vec<StepRun>,
    /// Steps in the flow.
    pub total: usize,
    /// The steps that were to run (1-based, inclusive): the whole flow unless a range was asked for.
    pub range: (usize, usize),
    /// The session state before each executed step and after the last one, by the number of the
    /// step it comes before (for resuming at that step).
    pub states: Vec<(usize, SessionState)>,
}

impl FlowRun {
    /// Steps in the range.
    pub fn planned(&self) -> usize {
        (self.range.1 + 1).saturating_sub(self.range.0)
    }

    pub fn passed(&self) -> bool {
        self.steps.len() == self.planned() && self.steps.iter().all(|s| s.run.passed())
    }

    /// Steps of the range not run because an earlier one failed.
    pub fn skipped(&self) -> usize {
        self.planned().saturating_sub(self.steps.len())
    }
}

/// Runs `flow` in `session`. `adjust` is applied to every request before it is sent.
pub async fn run_flow(
    session: &mut Session<'_>,
    flow: &FlowItem,
    adjust: impl Fn(&mut Request),
) -> FlowRun {
    run_flow_with(session, flow, adjust, None).await
}

/// [`run_flow`] with a pause of `delay` between steps (in addition to their `delay_ms`).
pub async fn run_flow_with(
    session: &mut Session<'_>,
    flow: &FlowItem,
    adjust: impl Fn(&mut Request),
    delay: Option<Duration>,
) -> FlowRun {
    run_flow_steps(session, flow, adjust, delay, &FlowSteps::default()).await
}

/// [`run_flow_with`] for the steps in `range`, starting from its saved state.
pub async fn run_flow_steps(
    session: &mut Session<'_>,
    flow: &FlowItem,
    adjust: impl Fn(&mut Request),
    delay: Option<Duration>,
    range: &FlowSteps,
) -> FlowRun {
    let total = flow.file.steps.len();
    let from = range.from.max(1);
    let to = range.to.unwrap_or(total).min(total);
    if let Some(start) = &range.start {
        session.restore(start);
    }
    let mut states = Vec::new();
    let path = session.collection().root.join(&flow.path);
    let mut steps = Vec::new();
    for (i, step) in flow.file.steps.iter().enumerate() {
        if i + 1 < from || i + 1 > to {
            continue;
        }
        states.push((i + 1, session.state()));
        if let Some(d) = delay
            && i + 1 > from
        {
            tokio::time::sleep(d).await;
        }
        if let Some(ms) = step.delay_ms {
            tokio::time::sleep(Duration::from_millis(ms)).await;
        }
        let inline = step.inline_request();
        let target = match (&step.request, &inline) {
            (Some(key), _) => Target::Request(key),
            (None, Some(file)) => Target::Inline {
                key: &flow.key,
                file,
                path: &path,
            },
            // Rejected when the flow is loaded.
            (None, None) => unreachable!("a step has `request` or `url`"),
        };
        let until = step
            .retry
            .as_ref()
            .and_then(|r| r.until.as_deref())
            .map(|u| (u, path.as_path()));
        let spec = SendSpec {
            target,
            lower_vars: Some(&flow.file.vars),
            upper_vars: Some(&step.vars),
            assert: (!step.assert.is_empty()).then_some((&*step.assert, path.as_path())),
            capture: (!step.capture.is_empty()).then_some(&step.capture),
            until,
            script: (!step.script.is_empty()).then_some((&step.script, path.as_path())),
        };

        let max = step.retry.as_ref().map_or(1, |r| r.max.max(1));
        let pause = Duration::from_millis(step.retry.as_ref().map_or(0, |r| r.every_ms));
        let mut attempts = 0;
        let run = loop {
            attempts += 1;
            let run = session.send_spec(spec, &adjust).await;
            let done = match &run.outcome {
                // A broken step does not get better by trying again.
                RunOutcome::Invalid(_) => true,
                RunOutcome::Failed(_) => false,
                RunOutcome::Response(_) if until.is_some() => run.until.iter().all(|o| o.passed),
                RunOutcome::Response(_) => run.passed(),
            };
            if done || attempts >= max {
                break run;
            }
            tokio::time::sleep(pause).await;
        };
        let passed = run.passed();
        steps.push(StepRun {
            number: i + 1,
            title: step.title(),
            attempts,
            run,
        });
        if !passed {
            break;
        }
        states.push((i + 2, session.state()));
    }
    // The state after a step is also the state before the next one: keep it once.
    states.dedup_by(|b, a| a.0 == b.0);
    FlowRun {
        key: flow.key.clone(),
        name: flow.name().to_owned(),
        steps,
        total,
        range: (from, to.max(from.saturating_sub(1))),
        states,
    }
}
