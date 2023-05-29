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
        .add_event::<manifest::ManifestSpawnEvent>()
        .add_system(manifest::update_dt)
        .add_system(manifest::read_messages)
        .add_system(manifest::manifest_spawner)
        .add_startup_system(startup)
        .run();

    /*
    let manifest = manifest::Manifest::parse(include_bytes!("manifest.json")).unwrap();

    let mut instance = Box::new(manifest.spawn());

    println!("Manifest {:#?}", &manifest);
    println!("Instance {:#?}", &instance);

    if let Some(inst) = instance.logic.as_mut() {
        inst.input
            .send(logic::runtime::LogicInputEvent::Update(0.3))
            .unwrap();
        inst.input
            .send(logic::runtime::LogicInputEvent::Close)
            .unwrap();

        while let Ok(out) = inst.output.try_recv() {
            println!("Got {:#?}", out);
        }
    }
     */
}
