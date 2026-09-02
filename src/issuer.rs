//! The control plane. In production this is a separate service: it holds the
//! root signing key, mints warrants for agents, and publishes revocation lists.
//!
//! The agent process never sees the root private key. It receives a minted
//! warrant plus its own holder key, and every enforcement point receives only
//! the root *public* key. This module stands in for that service so the demo
//! runs in one command; the boundary is the same.

use std::time::Duration;

use tenuo::sdk::prelude::*;
use tenuo::{constraints, Range};

pub struct ControlPlane {
    root: SigningKey,
}

impl ControlPlane {
    pub fn start() -> Self {
        Self { root: SigningKey::generate() }
    }

    /// What enforcement points are configured with. Safe to distribute.
    pub fn root_public_key(&self) -> PublicKey {
        self.root.public_key()
    }

    /// Mint the orchestrator's authority for one on-call shift.
    ///
    /// It may scale staging clusters to at most ten replicas and read any
    /// incident. It may not touch production, and it may not write.
    pub fn mint_orchestrator_warrant(&self, holder: &PublicKey) -> anyhow::Result<Warrant> {
        Ok(Warrant::builder()
            .capability(
                "scale_cluster",
                constraints! {
                    "cluster"  => Pattern::new("staging-*")?,
                    "replicas" => Range::max(10.0)?,
                },
            )
            .capability(
                "read_incident",
                constraints! { "incident_id" => Pattern::new("INC-*")? },
            )
            .capability("delegate_incident", constraints! { "incident_id" => Pattern::new("INC-*")? })
            .holder(holder.clone())
            .ttl(Duration::from_secs(600))
            .build(&self.root)?)
    }
}
