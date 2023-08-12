mod dom;
mod logic;
mod manifest;

use bevy::prelude::*;
use manifest::ManifestSpawnEvent;

fn startup(mut c: Commands, mut spawn_event: EventWriter<manifest::ManifestSpawnEvent>) {
    let e = c.spawn_empty();
    spawn_event.send(ManifestSpawnEvent {
        manifest: manifest::Manifest::parse(include_bytes!("manifest.json")).unwrap(),
        entity: e.id(),
    });
}

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(logic::LogicPlugin)
        .add_event::<manifest::ManifestSpawnEvent>()
        .add_systems(Update, manifest::manifest_spawner)
        .add_systems(Startup, startup)
        .run();
}
