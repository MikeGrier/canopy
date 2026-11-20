// Copyright (c) Michael Grier.

use super::*;

#[test]
fn registry_payload_uses_named_encoding_tags_and_spoken_values() {
    use acadia::coord::{HeldTiers, TierHold};
    let agent = Agent::new("id", "version", "description", 9);
    let bytes = registration_payload(
        &agent,
        HeldTiers {
            reader: Some(TierHold::Shared),
            writer: None,
        },
        false,
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["format_version"], registry_tags::FORMAT_VERSION);
    assert_eq!(value["reader_tier"], registry_tags::SHARED);
    assert!(value["writer_tier"].is_null());
    let bytes = registration_payload(
        &agent,
        HeldTiers {
            reader: None,
            writer: Some(TierHold::Exclusive),
        },
        false,
    )
    .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(value["reader_tier"].is_null());
    assert_eq!(value["writer_tier"], registry_tags::EXCLUSIVE);
    assert_eq!(value["agent_schema"], agent.schema_version());
}

#[test]
fn ordinary_admission_checks_each_layers_reader_range_and_writer_equality() {
    use acadia::coord::Access;
    for access in [Access::Read, Access::Write] {
        for spoken in [0, 1, 2, 3, 4, u32::MAX] {
            let agent = Agent::new("agent", "version", "description", spoken);
            let mut metadata = AgentMetadata::new(&agent);
            metadata.agent_schema = SchemaVersions {
                current: 3,
                oldest_reader: 1,
            };
            let allowed = if access == Access::Write {
                spoken == 3
            } else {
                (1..=3).contains(&spoken)
            };
            let result = metadata.check_admission(&agent, access);
            assert_eq!(result.is_ok(), allowed, "{access:?}, {spoken}");
            if !allowed {
                assert!(matches!(
                    result,
                    Err(StoreError::SchemaMismatch {
                        layer: SchemaLayer::Agent,
                        ..
                    })
                ));
            }
        }
        for (oldest_reader, current) in [(0, 0), (0, 1), (1, 1), (0, 2), (1, 2), (2, 2)] {
            let agent = Agent::new("agent", "version", "description", 1);
            let mut metadata = AgentMetadata::new(&agent);
            metadata.canopy_schema = SchemaVersions {
                current,
                oldest_reader,
            };
            let allowed = if access == Access::Write {
                CANOPY_SCHEMA_VERSION == current
            } else {
                (oldest_reader..=current).contains(&CANOPY_SCHEMA_VERSION)
            };
            let result = metadata.check_admission(&agent, access);
            assert_eq!(
                result.is_ok(),
                allowed,
                "{access:?}, {oldest_reader}, {current}"
            );
            if !allowed {
                assert!(matches!(
                    result,
                    Err(StoreError::SchemaMismatch {
                        layer: SchemaLayer::Canopy,
                        ..
                    })
                ));
            }
        }
    }
}

#[test]
fn schema_one_stores_are_not_admitted_by_schema_two() {
    use acadia::coord::Access;
    let agent = Agent::new("agent", "version", "description", 1);
    let mut metadata = AgentMetadata::new(&agent);
    metadata.canopy_schema = SchemaVersions {
        current: 1,
        oldest_reader: 1,
    };
    for access in [Access::Read, Access::Write] {
        assert!(matches!(
            metadata.check_admission(&agent, access),
            Err(StoreError::SchemaMismatch {
                layer: SchemaLayer::Canopy,
                spoken: 2,
                current: 1,
                oldest_reader: 1,
                ..
            })
        ));
    }
}

#[test]
fn metadata_preserves_opaque_agent_values_and_initializes_both_reader_floors() {
    for schema in [0, 1, 2, 3, 4, 7, 8, 16, 100, u32::MAX] {
        let agent = Agent::new("example.agent", "build-B", "agent documentation", schema);
        let metadata = AgentMetadata::new(&agent);
        assert_eq!(metadata.agent_identifier, agent.identifier());
        assert_eq!(metadata.agent_version, agent.version());
        assert_eq!(metadata.agent_description, agent.description());
        assert_eq!(metadata.agent_schema.current, agent.schema_version());
        assert_eq!(metadata.agent_schema.oldest_reader, schema);
        assert_eq!(metadata.canopy_schema.current, CANOPY_SCHEMA_VERSION);
        assert_eq!(metadata.canopy_schema.oldest_reader, CANOPY_SCHEMA_VERSION);
        let bytes = serde_json::to_vec(&metadata).unwrap();
        assert_eq!(
            serde_json::from_slice::<AgentMetadata>(&bytes).unwrap(),
            metadata
        );
    }
}
