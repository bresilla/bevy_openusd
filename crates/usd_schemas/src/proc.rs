//! `UsdProc` — procedural prim metadata.
//!
//! `UsdProcGenerativeProcedural` and its subclasses define prims that
//! a downstream engine (Houdini Engine, Renderman procedurals, …)
//! evaluates at render time. We can't execute the procedural without
//! the engine, but we can read the type identifier so the viewer can
//! at least show that the prim is procedural.
//!
//! The two attrs every procedural carries:
//!
//! - `info:procedural:type` (token) — tells the engine which
//!   procedural to invoke. Examples: `"HoudiniProcedural"`,
//!   `"RmanProcedural"`, etc.
//! - `proceduralSystem` (token, optional) — system identifier that
//!   selects which procedural-evaluation backend handles this prim.

use anyhow::Result;
use openusd::sdf::{Path, Value};

#[derive(Debug, Clone, Default)]
pub struct ReadProcedural {
    /// Authored `info:procedural:type`. Identifies the specific
    /// procedural to invoke.
    pub procedural_type: Option<String>,
    /// Authored `proceduralSystem`. Selects the evaluation backend.
    pub procedural_system: Option<String>,
}

pub fn read_procedural(stage: &openusd::Stage, prim: &Path) -> Result<Option<ReadProcedural>> {
    let procedural_type = read_token(stage, prim, "info:procedural:type")?;
    let procedural_system = read_token(stage, prim, "proceduralSystem")?;
    if procedural_type.is_none() && procedural_system.is_none() {
        return Ok(None);
    }
    Ok(Some(ReadProcedural {
        procedural_type,
        procedural_system,
    }))
}

fn read_token(stage: &openusd::Stage, prim: &Path, name: &str) -> Result<Option<String>> {
    let attr = prim.append_property(name).map_err(anyhow::Error::from)?;
    let v = stage
        .field::<Value>(attr, "default")
        .map_err(anyhow::Error::from)?;
    Ok(match v {
        Some(Value::Token(s)) | Some(Value::String(s)) => Some(s),
        _ => None,
    })
}
