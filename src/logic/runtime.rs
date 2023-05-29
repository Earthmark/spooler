use std::collections::BTreeMap;
use std::pin::Pin;
use std::rc::Rc;

use deno_core::error::AnyError;
use deno_core::futures::FutureExt;
use deno_core::url::Url;
use deno_core::v8;
use deno_core::JsRuntime;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::RuntimeOptions;

use tokio::runtime::Builder;
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
    Source { src: String },
}

impl LogicModule {
    fn resolve(&self) -> Result<Box<[u8]>, AnyError> {
        match self {
            LogicModule::Source { src } => Ok(src.as_bytes().to_vec().into_boxed_slice()),
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

#[derive(Debug)]
pub struct LogicInst {
    pub input: UnboundedSender<LogicInputEvent>,
    pub output: UnboundedReceiver<LogicOutputEvent>,
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
}

struct LogicRuntimeRoot {
    runtime: LogicRuntime,

    inputs: UnboundedReceiver<LogicInputEvent>,
    outputs: UnboundedSender<LogicOutputEvent>,
}

impl LogicRuntimeRoot {
    fn poll_runtime_proxy(
        module_loader: Logic,
        url: Url,
        mut inputs: UnboundedReceiver<LogicInputEvent>,
        outputs: UnboundedSender<LogicOutputEvent>,
    ) {
        let rt = Builder::new_current_thread().enable_all().build().unwrap();

        let local = LocalSet::new();
        local.spawn_local(async move {
            let maybe_runtime = LogicRuntime::spawn_runtime(module_loader, &url).await;
            match maybe_runtime {
                Ok(runtime) => {
                    let mut runtime_root = LogicRuntimeRoot {
                        runtime,
                        inputs,
                        outputs,
                    };

                    runtime_root.run_proxy().await;
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
        while let Some(msg) = self.inputs.recv().await {
            if let Err(e) = self.process_message(msg).await {
                self.inputs.close();
                let _ = self.outputs.send(LogicOutputEvent::Err(e));
            }
        }
        self.runtime.close().await;
    }

    async fn process_message(&mut self, input: LogicInputEvent) -> Result<(), AnyError> {
        match input {
            LogicInputEvent::Update(delta_t) => self.runtime.on_update(delta_t)?,
            LogicInputEvent::Close => self.inputs.close(),
        }
        Ok(())
    }
}

pub struct LogicRuntime {
    runtime: JsRuntime,
    this: v8::Global<v8::Object>,
    on_update: Option<v8::Global<v8::Function>>,
}

#[derive(Debug)]
pub enum LogicInputEvent {
    Update(f64),
    Close,
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
            runtime,
            on_update,
            this,
        })
    }

    pub fn on_update(&mut self, delta_t: f64) -> Result<(), deno_core::error::AnyError> {
        let scope = &mut self.runtime.handle_scope();
        if let Some(on_update) = self.on_update.as_ref() {
            let on_update = v8::Local::new(scope, on_update);
            let this = v8::Local::new(scope, &self.this);

            let delta_t_key = v8::String::new(scope, "deltaTime").unwrap();
            let delta_t = v8::Number::new(scope, delta_t);
            this.set(scope, delta_t_key.into(), delta_t.into());

            on_update.call(scope, this.into(), &[]);
        }

        Ok(())
    }

    pub async fn close(&mut self) {}
}
