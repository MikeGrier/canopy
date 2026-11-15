// Copyright (c) Michael Grier.

use super::*;
use std::borrow::Cow;

#[cfg(test)]
mod tests;

/// The canopy schema spoken by this build, independently of the genesis encoding version.
pub const CANOPY_SCHEMA_VERSION: u32 = 1;

/// Changing these values breaks the attachment payload encoding.
mod registry_tags {
    pub const FORMAT_VERSION: u32 = 1;
    pub const SHARED: &str = "shared";
    pub const EXCLUSIVE: &str = "exclusive";
}

#[derive(Serialize, Deserialize)]
struct AttachmentPayload<'agent> {
    format_version: u32,
    agent_identifier: Cow<'agent, str>,
    agent_version: Cow<'agent, str>,
    canopy_schema: u32,
    agent_schema: u32,
    reader_tier: Option<Cow<'agent, str>>,
    writer_tier: Option<Cow<'agent, str>>,
    process_id: u32,
    draining: bool,
}

/// One live attachment, decoded from canopy's versioned registry payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedHandle {
    pub token: acadia::coord::RegistryToken,
    pub agent_identifier: String,
    pub agent_version: String,
    pub canopy_schema: u32,
    pub agent_schema: u32,
    pub tiers: acadia::coord::HeldTiers,
    pub process_id: u32,
    pub draining: bool,
}

/// A diagnostic listing; liveness is as of each entry's probe, not a simultaneous snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachedHandles {
    pub live: Vec<AttachedHandle>,
    pub unknown: usize,
    pub may_be_incomplete: bool,
}

pub(super) fn attached_handles(listing: acadia::coord::Listing) -> AttachedHandles {
    let mut result = AttachedHandles {
        live: Vec::new(),
        unknown: listing.unknown,
        may_be_incomplete: listing.may_be_incomplete || listing.unknown != 0,
    };
    for entry in listing.live {
        let decoded = (|| {
            let payload: AttachmentPayload<'_> = serde_json::from_slice(&entry.payload).ok()?;
            if payload.format_version != registry_tags::FORMAT_VERSION {
                return None;
            }
            let mode = |tier| match tier {
                None => Some(None),
                Some(registry_tags::SHARED) => Some(Some(acadia::coord::TierHold::Shared)),
                Some(registry_tags::EXCLUSIVE) => Some(Some(acadia::coord::TierHold::Exclusive)),
                Some(_) => None,
            };
            let tiers = acadia::coord::HeldTiers {
                reader: mode(payload.reader_tier.as_deref())?,
                writer: mode(payload.writer_tier.as_deref())?,
            };
            if tiers != entry.tiers {
                return None;
            }
            Some(AttachedHandle {
                token: entry.token,
                agent_identifier: payload.agent_identifier.into_owned(),
                agent_version: payload.agent_version.into_owned(),
                canopy_schema: payload.canopy_schema,
                agent_schema: payload.agent_schema,
                tiers,
                process_id: payload.process_id,
                draining: payload.draining,
            })
        })();
        match decoded {
            Some(handle) => result.live.push(handle),
            None => {
                result.unknown = result.unknown.saturating_add(1);
                result.may_be_incomplete = true;
            }
        }
    }
    result
}

pub(super) fn registration_payload(
    agent: &Agent,
    tiers: acadia::coord::HeldTiers,
    draining: bool,
) -> Result<Vec<u8>, StoreError> {
    let mode = |tier| match tier {
        Some(acadia::coord::TierHold::Shared) => Some(Cow::Borrowed(registry_tags::SHARED)),
        Some(acadia::coord::TierHold::Exclusive) => Some(Cow::Borrowed(registry_tags::EXCLUSIVE)),
        None => None,
    };
    Ok(serde_json::to_vec(&AttachmentPayload {
        format_version: registry_tags::FORMAT_VERSION,
        agent_identifier: Cow::Borrowed(agent.identifier()),
        agent_version: Cow::Borrowed(agent.version()),
        canopy_schema: CANOPY_SCHEMA_VERSION,
        agent_schema: agent.schema_version(),
        reader_tier: mode(tiers.reader),
        writer_tier: mode(tiers.writer),
        process_id: std::process::id(),
        draining,
    })?)
}

/// The schema layer that decided admission.
/// Variant names are durable upgrade tags; changing them breaks the marker encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaLayer {
    /// Canopy's storage and coordination schema.
    Canopy,
    /// The agent's document schema.
    Agent,
}

/// The software opening a store and the document schema it speaks; strings are opaque to canopy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Agent {
    identifier: String,
    version: String,
    description: String,
    schema_version: u32,
}

impl Agent {
    /// Supplies the version-independent identity, software version, description and document schema.
    pub fn new(
        identifier: impl Into<String>,
        version: impl Into<String>,
        description: impl Into<String>,
        schema_version: u32,
    ) -> Self {
        Self {
            identifier: identifier.into(),
            version: version.into(),
            description: description.into(),
            schema_version,
        }
    }

    /// The version-independent software identity, compared exactly at open.
    pub fn identifier(&self) -> &str {
        &self.identifier
    }

    /// The software version, which canopy never orders.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Human-readable information about the software.
    pub fn description(&self) -> &str {
        &self.description
    }

    /// The document schema spoken by this agent.
    pub fn schema_version(&self) -> u32 {
        self.schema_version
    }
}

/// One layer's current schema and oldest admissible reader schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaVersions {
    /// The schema recorded for this layer.
    pub current: u32,
    /// The oldest schema permitted to read this layer.
    pub oldest_reader: u32,
}

/// Metadata recorded in the marker; changing these field names breaks its encoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct AgentMetadata {
    pub agent_identifier: String,
    pub agent_version: String,
    pub agent_description: String,
    pub canopy_schema: SchemaVersions,
    pub agent_schema: SchemaVersions,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition: Option<super::upgrade::Transition>,
}

impl AgentMetadata {
    pub(super) fn new(agent: &Agent) -> Self {
        Self {
            agent_identifier: agent.identifier.clone(),
            agent_version: agent.version.clone(),
            agent_description: agent.description.clone(),
            canopy_schema: SchemaVersions {
                current: CANOPY_SCHEMA_VERSION,
                oldest_reader: CANOPY_SCHEMA_VERSION,
            },
            agent_schema: SchemaVersions {
                current: agent.schema_version,
                oldest_reader: agent.schema_version,
            },
            transition: None,
        }
    }

    pub(super) fn check_identity(&self, agent: &Agent) -> Result<(), StoreError> {
        if self.agent_identifier != agent.identifier {
            return Err(StoreError::AgentMismatch {
                recorded: self.agent_identifier.clone(),
                requested: agent.identifier.clone(),
            });
        }
        Ok(())
    }

    pub(super) fn check_admission(
        &self,
        agent: &Agent,
        access: acadia::coord::Access,
    ) -> Result<(), StoreError> {
        self.check_identity(agent)?;
        let writable = access == acadia::coord::Access::Write;
        for (layer, spoken, schema) in [
            (
                SchemaLayer::Canopy,
                CANOPY_SCHEMA_VERSION,
                &self.canopy_schema,
            ),
            (
                SchemaLayer::Agent,
                agent.schema_version(),
                &self.agent_schema,
            ),
        ] {
            let transition = self
                .transition
                .as_ref()
                .filter(|transition| transition.request.layer == layer);
            let admitted = if let Some(transition) = transition {
                if writable {
                    spoken == transition.request.target
                } else {
                    transition.safe
                        && ((schema.oldest_reader..=schema.current).contains(&spoken)
                            || spoken == transition.request.target)
                }
            } else if writable {
                spoken == schema.current
            } else {
                (schema.oldest_reader..=schema.current).contains(&spoken)
            };
            if !admitted {
                return Err(StoreError::SchemaMismatch {
                    layer,
                    spoken,
                    current: schema.current,
                    oldest_reader: schema.oldest_reader,
                    writable,
                });
            }
        }
        Ok(())
    }
}
