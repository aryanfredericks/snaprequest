//! Runs: a flow or a list of requests, once or once per iteration / data row.

use std::time::Duration;

use futures_util::StreamExt as _;
use snapreq_collection::{Collection, Environment, FlowItem, Secrets, Variables};
use snapreq_core::{Client, Request};

use crate::flow::{FlowRun, run_flow_steps};
use crate::session::{RequestRun, Session};

/// What a run executes.
#[derive(Debug, Clone)]
pub enum Unit<'c> {
    Flow(&'c FlowItem),
    /// Requests by key, in order; every one is sent even if an earlier one failed.
    Requests(Vec<String>),
}

/// One pass over the unit.
#[derive(Debug)]
pub struct Iteration {
    /// 1-based.
    pub number: usize,
    /// The data row of this iteration, if the run is data-driven.
    pub data: Option<Variables>,
    pub result: UnitRun,
}

#[derive(Debug)]
pub enum UnitRun {
    Flow(FlowRun),
    /// The requests sent; fewer than `total` when the run bailed out.
    Requests {
        runs: Vec<RequestRun>,
        total: usize,
    },
}

impl Iteration {
    pub fn passed(&self) -> bool {
        match &self.result {
            UnitRun::Flow(f) => f.passed(),
            UnitRun::Requests { runs, total } => {
                runs.len() == *total && runs.iter().all(RequestRun::passed)
            }
        }
    }

    /// Every request sent (or attempted) in this iteration.
    pub fn requests(&self) -> Vec<&RequestRun> {
        match &self.result {
            UnitRun::Flow(f) => f.steps.iter().map(|s| &s.run).collect(),
            UnitRun::Requests { runs, .. } => runs.iter().collect(),
        }
    }
}

/// Everything a run needs besides the unit.
#[derive(Debug)]
pub struct Plan<'a> {
    pub collection: &'a Collection,
    pub environment: Option<&'a Environment>,
    pub secrets: &'a Secrets,
    pub client: Client,
    /// `--var` values.
    pub overrides: Variables,
    /// Values to hide in every request.
    pub hidden: Vec<String>,
    /// One iteration per row; empty for a run without data.
    pub rows: Vec<Variables>,
    /// Iterations when there are no rows (at least 1).
    pub iterations: usize,
    pub options: RunOptions,
    /// Live observers (trace and progress events).
    pub live: Option<crate::session::Live>,
    /// For flows: the steps to run and the state to start from (default: all, fresh).
    pub flow_steps: crate::flow::FlowSteps,
}

/// How a run proceeds.
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    /// Stop at the first failing request: the rest of the iteration and later
    /// iterations are skipped. (Flows always stop at a failing step.)
    pub bail: bool,
    /// Pause between requests (and flow steps) of an iteration.
    pub delay: Option<Duration>,
    /// Iterations run at the same time (at least 1). Requests within an iteration always
    /// run in order, because later ones may use values captured by earlier ones.
    pub parallel: usize,
}

/// The keys of every HTTP request of the collection, or of `folder` (a folder key such as
/// `users` or `users/admin`), in tree order. WebSocket requests are left out: they are opened
/// interactively, not run.
pub fn request_keys(collection: &Collection, folder: Option<&str>) -> Result<Vec<String>, String> {
    let mut all = collection.requests();
    all.retain(|(_, item)| item.file.kind() == snapreq_collection::RequestKind::Http);
    let keys: Vec<String> = match folder.map(|f| f.trim_matches('/')) {
        None | Some("") => all.into_iter().map(|(k, _)| k).collect(),
        Some(f) => {
            let prefix = format!("{f}/");
            all.into_iter()
                .map(|(k, _)| k)
                .filter(|k| k.starts_with(&prefix))
                .collect()
        }
    };
    if keys.is_empty() {
        return Err(match folder {
            Some(f) => format!("no requests in folder `{f}`"),
            None => "the collection has no requests".into(),
        });
    }
    Ok(keys)
}

/// What to run for `key`: the flow with that key, else the requests of the folder with
/// that key; `None` runs every request of the collection.
pub fn resolve_unit<'c>(collection: &'c Collection, key: Option<&str>) -> Result<Unit<'c>, String> {
    let Some(key) = key.map(|k| k.trim_matches('/')).filter(|k| !k.is_empty()) else {
        return request_keys(collection, None).map(Unit::Requests);
    };
    if let Some(flow) = collection.find_flow(key) {
        return Ok(Unit::Flow(flow));
    }
    request_keys(collection, Some(key))
        .map(Unit::Requests)
        .map_err(|_| {
            let flows: Vec<&str> = collection.flows.iter().map(|f| f.key.as_str()).collect();
            format!(
                "no flow or folder `{key}` in the collection (flows: {})",
                if flows.is_empty() {
                    "none".into()
                } else {
                    flows.join(", ")
                }
            )
        })
}

/// Runs `unit` once per row (or `plan.iterations` times). Each iteration starts with no
/// captured values. Results are in iteration order; with `bail`, iterations after a
/// failure are not started (ones already running finish).
pub async fn execute(
    plan: &Plan<'_>,
    unit: &Unit<'_>,
    adjust: impl Fn(&mut Request),
) -> Vec<Iteration> {
    let rows: Vec<Option<&Variables>> = if plan.rows.is_empty() {
        vec![None; plan.iterations.max(1)]
    } else {
        plan.rows.iter().map(Some).collect()
    };
    let parallel = plan.options.parallel.max(1);
    let mut pending = futures_util::stream::FuturesOrdered::new();
    let mut results = Vec::new();
    let mut next = rows.into_iter().enumerate();
    let mut stop = false;
    loop {
        while !stop && pending.len() < parallel {
            let Some((i, row)) = next.next() else { break };
            pending.push_back(iteration(plan, unit, i + 1, row, &adjust));
        }
        let Some(it) = pending.next().await else {
            break;
        };
        if plan.options.bail && !it.passed() {
            // Iterations already running finish; no new ones start.
            stop = true;
        }
        results.push(it);
    }
    results
}

async fn iteration(
    plan: &Plan<'_>,
    unit: &Unit<'_>,
    number: usize,
    row: Option<&Variables>,
    adjust: &impl Fn(&mut Request),
) -> Iteration {
    let mut session = Session::new(
        plan.collection,
        plan.environment,
        plan.secrets,
        plan.client.clone(),
    );
    session.overrides = plan.overrides.clone();
    for h in &plan.hidden {
        session.hide(h.clone());
    }
    if let Some(row) = row {
        session.data = row.clone();
    }
    session.iteration = number;
    if let Some(live) = &plan.live {
        session.trace = live.trace.clone();
        session.progress = live.progress.clone();
    }
    let delay = plan.options.delay;
    let result = match unit {
        Unit::Flow(flow) => {
            UnitRun::Flow(run_flow_steps(&mut session, flow, adjust, delay, &plan.flow_steps).await)
        }
        Unit::Requests(keys) => {
            let mut runs = Vec::new();
            for (i, key) in keys.iter().enumerate() {
                if let Some(d) = delay
                    && i > 0
                {
                    tokio::time::sleep(d).await;
                }
                let run = session.send(key, adjust).await;
                let failed = !run.passed();
                runs.push(run);
                if failed && plan.options.bail {
                    break;
                }
            }
            UnitRun::Requests {
                runs,
                total: keys.len(),
            }
        }
    };
    Iteration {
        number,
        data: row.cloned(),
        result,
    }
}
