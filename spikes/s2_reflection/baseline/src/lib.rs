//! Same components as the spike, serde only. Exists to measure compile cost.

mod components;
pub use components::*;

use box_core::world::{Entity, World};

/// Serialize every component of every entity (forces serde codegen to be used).
pub fn save(world: &World) -> serde_json::Result<String> {
    let mut out = Vec::new();
    for (e, uid) in world.query::<(Entity, &Uid)>().iter() {
        let mut m = serde_json::Map::new();
        m.insert("uid".into(), serde_json::to_value(uid)?);
        if let Some(c) = world.get::<Transform>(e) {
            m.insert("Transform".into(), serde_json::to_value(&*c)?);
        }
        if let Some(c) = world.get::<PointLight>(e) {
            m.insert("PointLight".into(), serde_json::to_value(&*c)?);
        }
        if let Some(c) = world.get::<Health>(e) {
            m.insert("Health".into(), serde_json::to_value(&*c)?);
        }
        if let Some(c) = world.get::<Npc>(e) {
            m.insert("Npc".into(), serde_json::to_value(&*c)?);
        }
        out.push(serde_json::Value::Object(m));
    }
    serde_json::to_string_pretty(&out)
}

pub fn load_npc(
    v: &serde_json::Value,
) -> serde_json::Result<(Transform, PointLight, Health, Npc, Name)> {
    Ok((
        serde_json::from_value(v["Transform"].clone())?,
        serde_json::from_value(v["PointLight"].clone())?,
        serde_json::from_value(v["Health"].clone())?,
        serde_json::from_value(v["Npc"].clone())?,
        serde_json::from_value(v["Name"].clone())?,
    ))
}
