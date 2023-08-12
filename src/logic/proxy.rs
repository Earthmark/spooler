use deno_core::futures::FutureExt;
use deno_core::ModuleSource;
use deno_core::{error::AnyError, url::Url};
use deno_core::{FastString, ModuleLoader, ModuleSpecifier, ModuleType, ResolutionKind};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::time::Instant;
use tokio::runtime::Builder;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::task::LocalSet;

use super::runtime::{LogicInputEvent, LogicOutputEvent, LogicRuntime};

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct Logic {
    modules: BTreeMap<Url, LogicModule>,
}

impl Logic {
    pub fn spawn(self) -> Result<LogicInst, AnyError> {
        LogicInst::new(self, Url::parse("local://main").unwrap())
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LogicModule {
    Direct { src: String },
}

impl LogicModule {
    fn resolve(&self) -> Result<FastString, AnyError> {
        match self {
            LogicModule::Direct { src } => Ok(src.to_owned().into()),
        }
    }
}

impl ModuleLoader for Logic {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<deno_core::ModuleSpecifier, bevy::asset::Error> {
        Ok(deno_core::resolve_import(specifier, referrer)?)
    }

    fn load(
        &self,
        module_specifier: &deno_core::ModuleSpecifier,
        _maybe_referrer: Option<&ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> Pin<Box<deno_core::ModuleSourceFuture>> {
        let module_specifier = module_specifier.clone();
        let modules = self.modules.clone();
        async move {
            modules
                .get(&module_specifier)
                .ok_or(deno_core::error::generic_error("Module not found"))
                .map(|code| {
                    ModuleSource::new(
                        ModuleType::JavaScript,
                        code.resolve().unwrap(),
                        &module_specifier,
                    )
                })
        }
        .boxed_local()
    }
}

type LogicInstOutput = LogicOutputEvent;

#[derive(Debug)]
pub struct LogicInstEvent {
    pub input: LogicInputEvent,
    pub send_time: Instant,
}

#[derive(Debug)]
pub struct LogicInst {
    input: UnboundedSender<LogicInstEvent>,
    pub output: UnboundedReceiver<LogicInstOutput>,
}

impl LogicInst {
    fn new(module_loader: Logic, url: Url) -> Result<Self, deno_core::error::AnyError> {
        let (in_send, in_recv) = unbounded_channel();
        let (out_send, out_recv) = unbounded_channel();

        std::thread::spawn(|| {
            LogicRuntimeRoot::poll_runtime_proxy(module_loader, url, in_recv, out_send);
        });

        Ok(Self {
            input: in_send,
            output: out_recv,
        })
    }

    pub fn send(&mut self, input: LogicInputEvent) -> Result<(), AnyError> {
        let send_time = Instant::now();
        self.input.send(LogicInstEvent { send_time, input })?;
        Ok(())
    }
}

struct LogicRuntimeRoot {
    runtime: LogicRuntime,

    inputs: UnboundedReceiver<LogicInstEvent>,
    outputs: UnboundedSender<LogicInstOutput>,
}

impl LogicRuntimeRoot {
    fn poll_runtime_proxy(
        module_loader: Logic,
        url: Url,
        mut inputs: UnboundedReceiver<LogicInstEvent>,
        outputs: UnboundedSender<LogicInstOutput>,
    ) {
        let rt = Builder::new_current_thread().enable_all().build().unwrap();

        let local = LocalSet::new();
        local.spawn_local(async move {
            match LogicRuntime::spawn_runtime(module_loader, &url).await {
                Ok(runtime) => {
                    LogicRuntimeRoot {
                        runtime,
                        inputs,
                        outputs,
                    }
                    .run_proxy()
                    .await;
                }
                Err(e) => {
                    inputs.close();
                    let _ = outputs.send(LogicOutputEvent::Err(e));
                }
            }
        });

        rt.block_on(local)
    }

    async fn run_proxy(&mut self) {
        loop {
            if let Err(e) = self.run_internal().await {
                self.runtime.close().await;
                self.inputs.close();
                let _ = self.outputs.send(LogicOutputEvent::Err(e));
                return;
            }
        }
    }

    async fn run_internal(&mut self) -> Result<(), AnyError> {
        self.runtime.run_event_loop().await?;

        match self.inputs.try_recv() {
            Ok(event) => self.runtime.process_message(event).await?,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => Err(TryRecvError::Disconnected)?,
        }
        Ok(())
    }
}
