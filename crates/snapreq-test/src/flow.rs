//! Running a flow: its steps in order, with delays and retries, stopping at the first
//! failing step.

use std::time::Duration;

use snapreq_collection::FlowItem;
use snapreq_core::Request;

use crate::session::{RequestRun, RunOutcome, SendSpec, Session, Target};

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
}

impl FlowRun {
    pub fn passed(&self) -> bool {
        self.steps.len() == self.total && self.steps.iter().all(|s| s.run.passed())
    }

    /// Steps not run because an earlier one failed.
    pub fn skipped(&self) -> usize {
        self.total - self.steps.len()
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
    let path = session.collection().root.join(&flow.path);
    let mut steps = Vec::new();
    for (i, step) in flow.file.steps.iter().enumerate() {
        if let Some(d) = delay
            && i > 0
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
    }
    FlowRun {
        key: flow.key.clone(),
        name: flow.name().to_owned(),
        steps,
        total: flow.file.steps.len(),
    }
}
