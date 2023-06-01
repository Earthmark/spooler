use std::collections::BTreeMap;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;
use std::time::Instant;

use deno_core::error::AnyError;
use deno_core::futures::FutureExt;
use deno_core::url::Url;
use deno_core::v8;
use deno_core::JsRuntime;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::RuntimeOptions;

use tokio::runtime::Builder;
use tokio::sync::mpsc::error::SendError;
use tokio::sync::mpsc::error::TryRecvError;
use tokio::sync::mpsc::unbounded_channel;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::LocalSet;

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct Logic {
    modules: BTreeMap<Url, LogicModule>,
}

impl Logic {
    pub fn spawn(&self) -> Result<LogicInst, AnyError> {
        LogicInst::new(&self, &Url::parse("local://main").unwrap())
    }
}

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LogicModule {
    Direct { src: String },
}

impl LogicModule {
    fn resolve(&self) -> Result<Box<[u8]>, AnyError> {
        match self {
            LogicModule::Direct { src } => Ok(src.as_bytes().to_vec().into_boxed_slice()),
        }
    }
}

impl ModuleLoader for Logic {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _is_main: bool,
    ) -> Result<deno_core::ModuleSpecifier, bevy::asset::Error> {
        Ok(deno_core::resolve_import(specifier, referrer)?)
    }

    fn load(
        &self,
        module_specifier: &deno_core::ModuleSpecifier,
        _maybe_referrer: Option<deno_core::ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> Pin<Box<deno_core::ModuleSourceFuture>> {
        let module_specifier = module_specifier.clone();
        let modules = self.modules.clone();
        async move {
            modules
                .get(&module_specifier)
                .ok_or(deno_core::error::generic_error("Module not found"))
                .map(|code| ModuleSource {
                    code: code.resolve().unwrap(),
                    module_type: deno_core::ModuleType::JavaScript,
                    module_url_specified: module_specifier.to_string(),
                    module_url_found: module_specifier.to_string(),
                })
        }
        .boxed_local()
    }
}

type LogicInstInput = (Instant, LogicInputEvent);
type LogicInstOutput = (LogicOutputEvent);

#[derive(Debug)]
pub struct LogicInst {
    input: UnboundedSender<LogicInstInput>,
    pub output: UnboundedReceiver<LogicInstOutput>,
}

impl LogicInst {
    fn new(module_loader: &Logic, url: &Url) -> Result<Self, deno_core::error::AnyError> {
        let (in_send, in_recv) = unbounded_channel();
        let (out_send, out_recv) = unbounded_channel();

        let module_loader = module_loader.clone();
        let url = url.clone();

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
        self.input.send((send_time, input))?;
        Ok(())
    }
}

struct LogicRuntimeRoot {
    runtime: LogicRuntime,

    inputs: UnboundedReceiver<LogicInstInput>,
    outputs: UnboundedSender<LogicInstOutput>,
}

impl LogicRuntimeRoot {
    fn poll_runtime_proxy(
        module_loader: Logic,
        url: Url,
        mut inputs: UnboundedReceiver<LogicInstInput>,
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
            Ok((send_time, msg)) => self.process_message(send_time, msg).await?,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => Err(TryRecvError::Disconnected)?,
        }
        Ok(())
    }

    async fn process_message(
        &mut self,
        send_time: Instant,
        input: LogicInputEvent,
    ) -> Result<(), AnyError> {
        match input {
            LogicInputEvent::Update(delta_t) => {
                let start = Instant::now();
                let runtime = self.runtime.on_update(delta_t)?;
                println!(
                    "Message delay: {:?}, took {:?}",
                    start.duration_since(send_time),
                    runtime
                );
            }
        }
        Ok(())
    }
}

pub struct LogicRuntime {
    update_counter: u64,
    runtime: JsRuntime,
    this: v8::Global<v8::Object>,
    on_update: Option<v8::Global<v8::Function>>,
}

#[derive(Debug)]
pub enum LogicInputEvent {
    Update(f64),
}

#[derive(Debug)]
pub enum LogicOutputEvent {
    Err(AnyError),
}

impl LogicRuntime {
    async fn spawn_runtime(
        module_loader: Logic,
        url: &Url,
    ) -> Result<Self, deno_core::error::AnyError> {
        let mut runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(Rc::new(module_loader)),
            ..Default::default()
        });

        let main_module = runtime.load_main_module(url, None).await?;
        let eval = runtime.mod_evaluate(main_module);
        runtime.run_event_loop(false).await?;
        eval.await??;

        let (on_update, this) = {
            let namespace = runtime.get_module_namespace(main_module)?;
            let scope = &mut runtime.handle_scope();
            let namespace = v8::Local::new(scope, namespace);

            let on_update_key = v8::String::new(scope, "OnUpdate").unwrap();
            let on_update = namespace.get(scope, on_update_key.into());
            let on_update = if let Some(on_update) = on_update {
                let on_update: v8::Local<v8::Function> = on_update.try_into()?;
                Some(v8::Global::new(scope, on_update))
            } else {
                None
            };

            let this = v8::Object::new(scope);
            let this = v8::Global::new(scope, this);

            (on_update, this)
        };

        Ok(Self {
            update_counter: 0,
            runtime,
            on_update,
            this,
        })
    }

    pub async fn run_event_loop(&mut self) -> Result<(), AnyError> {
        self.runtime.run_event_loop(false).await?;
        Ok(())
    }

    pub fn on_update(&mut self, delta_t: f64) -> Result<Duration, deno_core::error::AnyError> {
        let start = Instant::now();
        let scope = &mut self.runtime.handle_scope();
        if let Some(on_update) = self.on_update.as_ref() {
            let on_update = v8::Local::new(scope, on_update);
            let this = v8::Local::new(scope, &self.this);

            let delta_t_key = v8::String::new(scope, "deltaTime").unwrap();
            let delta_t = v8::Number::new(scope, delta_t);
            this.set(scope, delta_t_key.into(), delta_t.into());

            on_update.call(scope, this.into(), &[]);
        }

        self.update_counter += 1;

        Ok(Instant::now().duration_since(start))
    }

    pub async fn close(&mut self) {}
}
