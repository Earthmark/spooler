use std::{collections::BTreeMap, rc::Rc, sync::Arc};

use bevy::prelude::*;
use deno_core::{
    url::Url, FastString, JsRuntime, ModuleLoader, ModuleSource, ModuleSpecifier, ModuleType,
    ResolutionKind, RuntimeOptions,
};
use tokio::{
    runtime::Builder,
    sync::mpsc::{error::TryRecvError, unbounded_channel, UnboundedReceiver, UnboundedSender},
    task::LocalSet,
};

use crate::manifest::ManifestSection;

mod ecs;
mod expose;
mod feature;
mod func;
pub mod proxy;
pub mod runtime;

type Error = deno_core::error::AnyError;
type EResult<T> = Result<T, Error>;

pub struct LogicPlugin;

impl Plugin for LogicPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (logic_registration, logic_unregistration))
            .init_resource::<Manager>();
    }
}

fn logic_registration(query: Query<&Logic, Added<Logic>>, mut manager: ResMut<Manager>) {
    for logic in &query {
        manager.load_runtime(logic).unwrap();
    }
}

fn logic_unregistration(mut removed: RemovedComponents<Logic>, _manager: ResMut<Manager>) {
    for _logic in &mut removed {
        println!("Entity removed");
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct LogicManifest {
    modules: BTreeMap<Url, LogicModule>,
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LogicModule {
    Direct { src: Arc<str> },
}

impl ManifestSection for LogicManifest {
    type LiveInstance = Logic;

    fn spawn(&self) -> Self::LiveInstance {
        let source = match self.modules.get(&Url::parse("local://main").unwrap()) {
            Some(source) => source.clone(),
            None => panic!("a main module is required"),
        };
        Logic::new(source, vec![])
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

struct LogicArgs {
    source: LogicModule,
    features: Vec<Box<dyn Feature>>,
}

lazy_static::lazy_static! {
    static ref MAIN_URL: Url = Url::parse("local://main").unwrap();
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
    workers: Vec<Worker>,
    module_resolver: Arc<ModuleResolver>,
}

impl Manager {
    fn load_runtime(&mut self, logic: &Logic) -> EResult<RuntimeConnection> {
        if self.workers.is_empty() {
            self.workers.push(Worker::new(self.module_resolver.clone()));
        }
        let worker = self.workers.get_mut(0).unwrap();
        worker.create_runtime(logic.args.clone())
    }
}

#[derive(Default)]
struct ModuleResolver {
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

enum WorkerInputEvent {
    Create(Arc<LogicArgs>),
}

struct Worker {
    inputs: UnboundedSender<WorkerInputEvent>,
}

impl Worker {
    fn new(module_resolver: Arc<ModuleResolver>) -> Self {
        let (sender, recv) = unbounded_channel();
        WorkerImpl::spawn(module_resolver, recv);
        Worker { inputs: sender }
    }

    fn create_runtime(&mut self, args: Arc<LogicArgs>) -> EResult<RuntimeConnection> {
        // Extend this to load balance.
        self.inputs.send(WorkerInputEvent::Create(args))?;
        Ok(RuntimeConnection)
    }
}

struct WorkerImpl {
    control_src: UnboundedReceiver<WorkerInputEvent>,
    module_resolver: Arc<ModuleResolver>,
    runtimes: Vec<Runtime>,
}

impl WorkerImpl {
    fn spawn(
        module_resolver: Arc<ModuleResolver>,
        control_src: UnboundedReceiver<WorkerInputEvent>,
    ) {
        std::thread::spawn(|| {
            let local = LocalSet::new();
            local.spawn_local(
                WorkerImpl {
                    control_src,
                    module_resolver,
                    runtimes: default(),
                }
                .main(),
            );
            Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(local)
        });
    }

    async fn main(mut self) {
        loop {
            match self.control_src.try_recv() {
                Ok(WorkerInputEvent::Create(arg)) => {
                    if let Ok(runtime) = Runtime::new(self.module_resolver.clone(), arg).await {
                        self.runtimes.push(runtime);
                    }
                }
                Err(TryRecvError::Disconnected) => todo!("shutdown"),
                Err(TryRecvError::Empty) => {} // nothing to change.
            }

            for runtime in &mut self.runtimes {
                runtime.run_event_loop().await.unwrap();
            }
        }
    }
}

struct RuntimeConnection;

struct Runtime {
    features: Vec<Box<dyn FeatureInstance>>,
    runtime: JsRuntime,
    status: RuntimeStatus,
}

enum RuntimeStatus {
    Running,
    Faulted,
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

    async fn run_event_loop(&mut self) -> EResult<()> {
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
