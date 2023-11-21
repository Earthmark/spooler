use std::{collections::BTreeMap, rc::Rc, sync::Arc};

use bevy::prelude::*;
use deno_core::{
    url::Url, FastString, JsRuntime, ModuleLoader, ModuleSource, ModuleSpecifier, ModuleType,
    ResolutionKind, RuntimeOptions,
};

use crate::manifest::ManifestSection;

mod expose;
mod interop;
mod worker;

type Error = deno_core::error::AnyError;
type EResult<T> = Result<T, Error>;

pub struct LogicPlugin;

impl Plugin for LogicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (logic_registration, logic_removal))
            .init_resource::<Manager>();
    }
}

fn logic_registration(query: Query<&Logic, Added<Logic>>, mut manager: ResMut<Manager>) {
    for logic in &query {
        manager.load_runtime(logic).unwrap();
    }
}

fn logic_removal(mut removed: RemovedComponents<Logic>, _manager: ResMut<Manager>) {
    for _logic in &mut removed {
        println!("Entity removed");
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct LogicManifest {
    main: LogicModule,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LogicModule {
    Direct { src: Arc<str> },
}

impl ManifestSection for LogicManifest {
    type LiveInstance = Logic;

    fn spawn(&self) -> Self::LiveInstance {
        Logic::new(self.main.clone(), vec![])
    }
}

#[derive(Component)]
pub struct Logic {
    args: Arc<LogicArgs>,
}

impl Logic {
    fn new(source: LogicModule, features: Vec<Box<dyn Feature>>) -> Self {
        Self {
            args: Arc::new(LogicArgs { source, features }),
        }
    }
}

pub struct LogicArgs {
    source: LogicModule,
    features: Vec<Box<dyn Feature>>,
}

lazy_static::lazy_static! {
    static ref MAIN_URL: Url = Url::parse("builtin://main").unwrap();
}

impl LogicModule {
    fn direct_url(&self) -> &Url {
        match self {
            LogicModule::Direct { src: _ } => &MAIN_URL,
        }
    }

    fn source(&self) -> &Arc<str> {
        match self {
            LogicModule::Direct { src } => &src,
        }
    }
}

trait Feature: Sync + Send {
    fn init(&self, runtime: &mut Runtime) -> EResult<Box<dyn FeatureInstance>>;
}

trait FeatureInstance {
    fn update(&self, runtime: &mut Runtime) -> EResult<()>;
}

#[derive(Default, Resource)]
struct Manager {
    workers: Vec<worker::Worker>,
    module_resolver: Arc<ModuleResolver>,
}

impl Manager {
    fn load_runtime(&mut self, logic: &Logic) -> EResult<RuntimeConnection> {
        if self.workers.is_empty() {
            self.workers
                .push(worker::Worker::new(self.module_resolver.clone()));
        }
        let worker = self.workers.get_mut(0).unwrap();
        worker.create_runtime(logic.args.clone())
    }
}

#[derive(Default)]
pub struct ModuleResolver {
    fallback_protocols: BTreeMap<&'static str, ProtocolResolver>,
}

impl ModuleResolver {
    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        referrer: Option<&ModuleSpecifier>,
        is_dyn_import: bool,
    ) -> ModuleLoadResult {
        match module_specifier.scheme() {
            "http" => todo!(),
            "https" => todo!(),
            "local" => todo!(),
            scheme => self.fallback_protocols.get(scheme).unwrap().load(
                module_specifier,
                referrer,
                is_dyn_import,
            ),
        }
    }
}

enum ProtocolResolver {}

impl ProtocolResolver {
    fn load(
        &self,
        _module_specifier: &ModuleSpecifier,
        _referrer: Option<&ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> ModuleLoadResult {
        todo!("Resolve protocols");
    }
}

pub struct RuntimeConnection;

struct Runtime {
    features: Vec<Box<dyn FeatureInstance>>,
    runtime: JsRuntime,
    status: RuntimeStatus,
}

enum RuntimeStatus {
    Running,
    Faulted,
}

impl RuntimeStatus {
    fn should_update(&self) -> bool {
        match self {
            RuntimeStatus::Running => true,
            RuntimeStatus::Faulted => false,
        }
    }
}

impl Runtime {
    async fn new(global_resolver: Arc<ModuleResolver>, args: Arc<LogicArgs>) -> EResult<Self> {
        let mut runtime = Runtime {
            runtime: JsRuntime::new(RuntimeOptions {
                module_loader: Some(Rc::new(RuntimeResolver {
                    global_resolver,
                    source: args.source.source().clone(),
                })),
                ..default()
            }),
            features: Vec::with_capacity(args.features.len()),
            status: RuntimeStatus::Running,
        };

        for feature in &args.features {
            let inst = feature.init(&mut runtime)?;
            runtime.features.push(inst);
        }

        let main_module = runtime
            .runtime
            .load_main_module(args.source.direct_url(), None)
            .await?;
        let loaded = runtime.runtime.mod_evaluate(main_module);
        runtime.runtime.run_event_loop(false).await?;
        loaded.await??;

        Ok(runtime)
    }

    async fn run_event_loop(&mut self) {
        if !self.status.should_update() {
            return;
        }

        if let Err(_e) = self.run_event_loop_internal().await {
            self.status = RuntimeStatus::Faulted;
        }
    }

    async fn run_event_loop_internal(&mut self) -> EResult<()> {
        self.runtime.run_event_loop(false).await?;
        Ok(())
    }
}

struct RuntimeResolver {
    global_resolver: Arc<ModuleResolver>,
    source: Arc<str>,
}

type ModuleLoadResult = std::pin::Pin<Box<deno_core::ModuleSourceFuture>>;

impl ModuleLoader for RuntimeResolver {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, bevy::asset::Error> {
        Ok(deno_core::resolve_import(specifier, referrer)?)
    }

    fn load(
        &self,
        module_specifier: &ModuleSpecifier,
        maybe_referrer: Option<&ModuleSpecifier>,
        is_dyn_import: bool,
    ) -> ModuleLoadResult {
        if module_specifier == &*MAIN_URL {
            let module = ModuleSource::new(
                ModuleType::JavaScript,
                FastString::Arc(self.source.clone()),
                module_specifier,
            );
            Box::pin(async { Ok(module) })
        } else {
            self.global_resolver
                .load(module_specifier, maybe_referrer, is_dyn_import)
        }
    }
}
