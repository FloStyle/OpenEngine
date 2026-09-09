//! The **Operator seam** (ADR-0002 / spec 51-52): one observe → propose →
//! verify → apply loop shared by the human, the wasm game logic and an AI.
//!
//! This module provides the typed *proposal* half: a model (or agent) replies
//! in "the language of the engine" — a JSON [`ProposeBatch`] of [`ProposeOp`]s —
//! which the engine parses, validates and (via the harness `/transaction`) can
//! apply reversibly. It deliberately is **not** a full agent loop (ADR-0002
//! non-goals): it only defines the typed proposal contract + the parse that
//! turns an untrusted model string into machine-usable ops.
//!
//! # Wire shape
//!
//! The ops map 1:1 onto the harness mutation endpoints so apply reuses the
//! existing reversible `/transaction` channel:
//!
//! ```json
//! { "ops": [
//!   { "op": "spawn", "transform": [x,y,z], "scale": [s,s,s], "color": [r,g,b,a] },
//!   { "op": "set", "entity": 2, "component": "transform", "value": [x,y,z] },
//!   { "op": "despawn", "entity": 0 }
//! ]}
//! ```

use serde::{Deserialize, Serialize};

/// A single typed engine mutation an operator proposes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum ProposeOp {
    /// Spawn a new entity. `color` is `[r,g,b,a]` 0..=255; scale optional.
    Spawn {
        /// World position `[x,y,z]`.
        #[serde(default)]
        transform: [f32; 3],
        /// Scale `[x,y,z]`, default all-ones.
        #[serde(default = "ones")]
        scale: [f32; 3],
        /// RGBA color `[r,g,b,a]`, default opaque white.
        #[serde(default = "white")]
        color: [u8; 4],
    },
    /// Overwrite one component of an existing entity (`transform`/`scale`/`color`).
    Set {
        /// Entity index.
        entity: u32,
        /// Component name.
        component: String,
        /// Component values (floats; color allowed too).
        value: Vec<f32>,
    },
    /// Remove an entity.
    Despawn {
        /// Entity index.
        entity: u32,
    },
}

fn ones() -> [f32; 3] {
    [1.0, 1.0, 1.0]
}
fn white() -> [u8; 4] {
    [255, 255, 255, 255]
}

/// A batch of typed engine mutations proposed by an operator.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ProposeBatch {
    /// Ordered ops; applied atomically (roll back on any failure).
    pub ops: Vec<ProposeOp>,
}

impl ProposeBatch {
    /// An empty batch (no-op; useful as a safe default).
    pub fn empty() -> Self {
        ProposeBatch { ops: Vec::new() }
    }
}

/// Parse + validate an untrusted model reply as a typed proposal.
///
/// Accepts the wire shape `{ "ops": [ ... ] }`. Returns a typed error (never
/// panics) describing the first parse/validation problem so the model can fix
/// it without reading engine source.
pub fn parse_proposal(json: &str) -> Result<ProposeBatch, ProposeError> {
    let batch: ProposeBatch =
        serde_json::from_str(json).map_err(|e| ProposeError::Parse(e.to_string()))?;
    validate(&batch)?;
    Ok(batch)
}

/// Error parsing/validating a proposal (typed, model-actionable).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProposeError {
    /// The reply was not valid proposal JSON.
    Parse(String),
    /// Structurally valid but violates an invariant.
    Invalid(String),
}

impl std::fmt::Display for ProposeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProposeError::Parse(m) => write!(f, "parse: {m}"),
            ProposeError::Invalid(m) => write!(f, "invalid: {m}"),
        }
    }
}
impl std::error::Error for ProposeError {}

/// Validate a batch's invariants (colors in range, set has a value, etc.).
fn validate(batch: &ProposeBatch) -> Result<(), ProposeError> {
    for (i, op) in batch.ops.iter().enumerate() {
        match op {
            ProposeOp::Spawn { .. } => {
                // color is [u8;4] so serde already rejects channels outside 0..=255.
            }
            ProposeOp::Set {
                value, component, ..
            } => {
                if value.is_empty() {
                    return Err(ProposeError::Invalid(format!("op {i}: set value empty")));
                }
                let allowed = ["transform", "scale", "color"];
                if !allowed.contains(&component.as_str()) {
                    return Err(ProposeError::Invalid(format!(
                        "op {i}: unknown component '{component}' (use transform|scale|color)"
                    )));
                }
            }
            ProposeOp::Despawn { .. } => {}
        }
    }
    Ok(())
}

/// Build a compact, human/agent-readable observe context from a world summary
/// (entity_count + per-entity transform/color) plus schema hints. Fed to the
/// model as the system prompt so it can propose in-engine changes.
pub fn observe_context(entity_count: usize, entities: &[EntitySummary]) -> String {
    let mut s = format!("OpenEngine live world: {entity_count} entit(ies).\n");
    for e in entities {
        s.push_str(&format!(
            "  #{} at {:?} color {:?}\n",
            e.index, e.transform, e.color
        ));
    }
    s.push_str(
        "\nTo change the world, reply with ONLY a JSON batch, e.g.\n\
         {\"ops\":[{\"op\":\"spawn\",\"transform\":[1,0,0],\"color\":[255,0,0,255]}, \
         {\"op\":\"set\",\"entity\":2,\"component\":\"transform\",\"value\":[2,0,0]}, \
         {\"op\":\"despawn\",\"entity\":0}]}\n",
    );
    s
}

/// A lightweight per-entity summary for the observe context (no engine deps).
#[derive(Clone, Debug)]
pub struct EntitySummary {
    /// Entity index.
    pub index: u32,
    /// World position `[x,y,z]`.
    pub transform: [f32; 3],
    /// RGBA color.
    pub color: [u8; 4],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_spawn_batch() {
        let b =
            parse_proposal(r#"{"ops":[{"op":"spawn","transform":[1,0,0],"color":[255,0,0,255]}]}"#)
                .unwrap();
        assert_eq!(b.ops.len(), 1);
        match &b.ops[0] {
            ProposeOp::Spawn {
                transform,
                color,
                scale,
            } => {
                assert_eq!(*transform, [1.0, 0.0, 0.0]);
                assert_eq!(*color, [255, 0, 0, 255]);
                assert_eq!(*scale, [1.0, 1.0, 1.0]); // default
            }
            _ => panic!("expected spawn"),
        }
    }

    #[test]
    fn parses_mixed_batch() {
        let b = parse_proposal(
            r#"{"ops":[
                {"op":"spawn","transform":[0,1,0]},
                {"op":"set","entity":2,"component":"transform","value":[2,0,0]},
                {"op":"despawn","entity":0}
            ]}"#,
        )
        .unwrap();
        assert_eq!(b.ops.len(), 3);
    }

    #[test]
    fn rejects_unknown_component() {
        let e = parse_proposal(r#"{"ops":[{"op":"set","entity":0,"component":"hp","value":[5]}]}"#)
            .unwrap_err();
        assert!(matches!(e, ProposeError::Invalid(_)));
    }

    #[test]
    fn rejects_color_out_of_range() {
        assert!(parse_proposal(r#"{"ops":[{"op":"spawn","color":[300,0,0,255]}]}"#).is_err());
    }

    #[test]
    fn rejects_bad_json() {
        let e = parse_proposal("not json").unwrap_err();
        assert!(matches!(e, ProposeError::Parse(_)));
    }

    #[test]
    fn observe_context_mentions_ops_schema() {
        let ctx = observe_context(
            1,
            &[EntitySummary {
                index: 0,
                transform: [1.0, 0.0, 0.0],
                color: [255, 0, 0, 255],
            }],
        );
        assert!(ctx.contains("entit(ies)"));
        assert!(ctx.contains("\"ops\""));
        assert!(ctx.contains("#0"));
    }
}
