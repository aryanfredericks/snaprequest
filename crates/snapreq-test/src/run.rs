//! Runs: a flow or a list of requests, once or once per iteration / data row.

use snapreq_collection::{Collection, Environment, FlowItem, Secrets, Variables};
use snapreq_core::{Client, Request};

use crate::flow::{FlowRun, run_flow};
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
    Requests(Vec<RequestRun>),
}

impl Iteration {
    pub fn passed(&self) -> bool {
        match &self.result {
            UnitRun::Flow(f) => f.passed(),
            UnitRun::Requests(r) => r.iter().all(RequestRun::passed),
        }
    }

    /// Every request sent (or attempted) in this iteration.
    pub fn requests(&self) -> Vec<&RequestRun> {
        match &self.result {
            UnitRun::Flow(f) => f.steps.iter().map(|s| &s.run).collect(),
            UnitRun::Requests(r) => r.iter().collect(),
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
}

/// The keys of every request of the collection, or of `folder` (a folder key such as
/// `users` or `users/admin`), in tree order.
pub fn request_keys(collection: &Collection, folder: Option<&str>) -> Result<Vec<String>, String> {
    let all = collection.requests();
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

/// Runs `unit` once per row (or `plan.iterations` times). Each iteration starts with no
/// captured values.
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
    let mut out = Vec::new();
    for (i, row) in rows.into_iter().enumerate() {
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
        let result = match unit {
            Unit::Flow(flow) => UnitRun::Flow(run_flow(&mut session, flow, &adjust).await),
            Unit::Requests(keys) => {
                let mut runs = Vec::new();
                for key in keys {
                    runs.push(session.send(key, &adjust).await);
                }
                UnitRun::Requests(runs)
            }
        };
        out.push(Iteration {
            number: i + 1,
            data: row.cloned(),
            result,
        });
    }
    out
}
