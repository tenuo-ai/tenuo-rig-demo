//! Give a worker less authority than the orchestrator holds. Never more.

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use tenuo::sdk::prelude::*;
use tenuo::{constraints, Exact, Pattern, Range};

use crate::authority::RunAuthority;

/// A child authority that can read exactly one incident and nothing else.
///
/// The guard checks the parent under current policy before minting, so a
/// revoked or expired orchestrator cannot hand out children.
pub fn child_for_incident(parent: &RunAuthority, incident_id: &str) -> anyhow::Result<RunAuthority> {
    let profile = DelegationProfile::new()
        .capability(
            "read_incident",
            constraints! { "incident_id" => Exact::new(incident_id) },
        )
        .ttl(Duration::from_secs(300))
        .terminal();

    let child = parent
        .guard
        .delegate(&parent.authority, &profile)
        .context("delegate to incident worker")?;

    Ok(RunAuthority {
        guard: parent.guard.clone(),
        authority: Arc::new(child),
        run_id: format!("{}/{}", parent.run_id, incident_id),
    })
}

/// Try to hand a child a higher replica cap than the parent holds. Must fail.
pub fn child_with_replicas(parent: &RunAuthority, max_replicas: f64) -> anyhow::Result<RunAuthority> {
    let profile = DelegationProfile::new()
        .capability(
            "scale_cluster",
            constraints! {
                "cluster"  => Pattern::new("staging-*")?,
                "replicas" => Range::max(max_replicas)?,
            },
        )
        .ttl(Duration::from_secs(300));
    let child = parent
        .guard
        .delegate(&parent.authority, &profile)
        .context("delegate scale_cluster")?;
    Ok(RunAuthority { guard: parent.guard.clone(), authority: Arc::new(child), run_id: format!("{}/scaler", parent.run_id) })
}
