use crate::dom::Dom;
use crate::logic::runtime::{Logic, LogicInst};
use bevy::ecs::system::EntityCommands;
use bevy::prelude::*;

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Manifest {
    dom: Option<Dom>,
    logic: Option<Logic>,
}

#[derive(Component)]
pub struct LogicComponent(LogicInst);

impl Manifest {
    pub fn parse(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    pub fn spawn(&self, c: &mut EntityCommands) {
        // if let Some(dom) = self.dom.as_ref() {}
        if let Some(logic) = self.logic.as_ref() {
            c.insert(LogicComponent(logic.clone().spawn().unwrap()));
        }
    }
}

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

pub fn update_dt(mut query: Query<&mut LogicComponent>, time: Res<Time>) {
    for mut logic in query.iter_mut() {
        let _ = logic
            .0
            .send(crate::logic::runtime::LogicInputEvent::Update(
                time.delta_seconds_f64(),
            ));
    }
}

pub fn read_messages(mut query: Query<&mut LogicComponent>) {
    for mut logic in query.iter_mut() {
        while let Ok(msg) = logic.0.output.try_recv() {
            println!("Msg from logic {:#?}", msg);
        }
    }
}
