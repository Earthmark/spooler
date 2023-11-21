use crate::dom::Dom;
use crate::logic::LogicManifest;
use bevy::ecs::system::EntityCommands;
use bevy::prelude::*;

pub trait ManifestSection {
    type LiveInstance;
    fn spawn(&self) -> Self::LiveInstance;
}

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Manifest {
    dom: Option<Dom>,
    logic: Option<LogicManifest>,
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    pub fn spawn(&self, c: &mut EntityCommands) {
        // if let Some(dom) = self.dom.as_ref() {}
        if let Some(logic) = self.logic.as_ref() {
            c.insert(logic.spawn());
        }
    }
}

#[derive(Event)]
pub struct ManifestSpawnEvent {
    pub manifest: Manifest,
    pub entity: Entity,
}

pub fn manifest_spawner(mut spawn_event: EventReader<ManifestSpawnEvent>, mut c: Commands) {
    for ev in spawn_event.iter() {
        if let Some(mut e) = c.get_entity(ev.entity) {
            ev.manifest.spawn(&mut e);
        }
    }
}
