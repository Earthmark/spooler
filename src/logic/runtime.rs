use std::collections::BTreeMap;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;
use std::time::Instant;

use deno_core::error::AnyError;
use deno_core::futures::FutureExt;
use deno_core::url::Url;
use deno_core::v8;
use deno_core::v8::HandleScope;
use deno_core::JsRuntime;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::RuntimeOptions;

use tokio::runtime::Builder;
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

type LogicInstOutput = LogicOutputEvent;

#[derive(Debug)]
struct LogicInstEvent {
    input: LogicInputEvent,
    send_time: Instant,
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
            Ok(event) => self.process_message(event).await?,
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => Err(TryRecvError::Disconnected)?,
        }
        Ok(())
    }

    async fn process_message(&mut self, event: LogicInstEvent) -> Result<(), AnyError> {
        match event.input {
            LogicInputEvent::Update(delta_t) => {
                let start = Instant::now();
                let runtime = self.runtime.on_update(delta_t)?;
                println!(
                    "Message delay: {:?}, took {:#?}",
                    start.duration_since(event.send_time),
                    runtime
                );
            }
        }
        Ok(())
    }
}

pub trait RuntimeExposed: Sized {
    fn setup_template(template: &mut ExposedTypeBinder<Self>);
}

pub fn create_runtime_template<'a, Exposed>(
    scope: &'a mut v8::HandleScope<'a>,
) -> (
    v8::Local<'a, v8::ObjectTemplate>,
    RuntimeExposedBinding<Exposed>,
)
where
    Exposed: RuntimeExposed,
{
    let template = v8::ObjectTemplate::new(scope);
    template.set_internal_field_count(1);

    let mut binder = ExposedTypeBinder {
        scope,
        template: &template,
        binding: RuntimeExposedBinding {
            field_num: 0,
            _t: Default::default(),
        },
    };

    Exposed::setup_template(&mut binder);

    (template, binder.binding)
}

pub struct RuntimeExposedBinding<Type> {
    field_num: usize,
    _t: PhantomData<Type>,
}

impl<Type> RuntimeExposedBinding<Type> {
    pub fn set_internal_field(
        &self,
        scope: &mut v8::HandleScope,
        obj: &mut Box<Type>,
        target: v8::Local<v8::Object>,
    ) {
        let external = v8::External::new(scope, &mut **obj as *mut Type as *mut c_void);
        target.set_internal_field(self.field_num, external.into());
    }
}

pub struct ExposedTypeBinder<'a, 'b, BoundType> {
    scope: &'a mut v8::HandleScope<'a>,
    template: &'b v8::Local<'a, v8::ObjectTemplate>,
    binding: RuntimeExposedBinding<BoundType>,
}

impl<BoundType: RuntimeExposed> ExposedTypeBinder<'_, '_, BoundType> {
    fn set_accessor<Getter, AccessorResult>(&mut self, name: &str, getter: Getter)
    where
        Getter: Fn(&BoundType, &mut v8::HandleScope) -> AccessorResult,
        AccessorResult : ExposedReturnable,
    {
        let scope = &mut self.scope;
        let name = v8::String::new(scope, name).unwrap();

        let field_num = self.binding.field_num;
        self.template.set_accessor(
            name.into(),
            |scope: &mut v8::HandleScope,
             _key: v8::Local<v8::Name>,
             args: v8::PropertyCallbackArguments,
             rv: v8::ReturnValue| {
                let this = args.this();
                let local = Self::get_external(scope, &this, field_num);
                let result = getter(local, scope);
                result.set(rv);
            },
        );
    }

    fn get_external<'a, T>(
        scope: &mut v8::HandleScope,
        obj: &v8::Local<'a, v8::Object>,
        field_num: usize,
    ) -> &'a T {
        let external = obj.get_internal_field(scope, field_num).unwrap();
        let external = unsafe { v8::Local::<v8::External>::cast(external) };
        unsafe { &*(external.value() as *const T) }
    }
}

impl RuntimeExposed for RuntimeHandles {
    fn setup_template(template: &mut ExposedTypeBinder<Self>) {
        template.set_accessor("deltaTime", |s, _scope| *s.delta_t);
    }
}

trait ExposedReturnable {
    fn set(self, rv: v8::ReturnValue);
}

impl ExposedReturnable for f64 {
    fn set(self, mut rv: v8::ReturnValue) {
        rv.set_double(self);
    }
}

impl ExposedReturnable for bool {
    fn set(self, mut rv: v8::ReturnValue) {
        rv.set_bool(self);
    }
}

impl ExposedReturnable for i32 {
    fn set(self, mut rv: v8::ReturnValue) {
        rv.set_int32(self);
    }
}

impl ExposedReturnable for u32 {
    fn set(self, mut rv: v8::ReturnValue) {
        rv.set_uint32(self);
    }
}

impl<'cb> ExposedReturnable for v8::Local<'cb, v8::Value> {
    fn set(self, mut rv: v8::ReturnValue) {
        rv.set(self);
    }
}

impl<T : ExposedReturnable> ExposedReturnable for Option<T> {
    fn set(self, mut rv: v8::ReturnValue) {
        match self {
            Some(inner) => inner.set(rv),
            None => rv.set_null()
        };
    }
}

struct LogicRuntime {
    update_counter: u64,
    runtime: JsRuntime,
    handles: RuntimeHandles,
}

struct RuntimeHandles {
    delta_t: Box<f64>,
    this: v8::Global<v8::Object>,
    on_update: Option<v8::Global<v8::Function>>,
}

impl RuntimeHandles {
    fn new<'a>(
        runtime: &mut JsRuntime,
        namespace: &v8::Global<v8::Object>,
    ) -> Result<Self, AnyError> {
        let scope = &mut runtime.handle_scope();
        let namespace = &v8::Local::new(scope, namespace);

        let this_template = v8::ObjectTemplate::new(scope);
        this_template.set_internal_field_count(1);

        let delta_t_key = v8::String::new(scope, "deltaTime").unwrap();
        this_template.set_accessor(delta_t_key.into(), Self::get_delta_t);

        let this = this_template.new_instance(scope).unwrap();
        let delta_t = Self::assign_delta_t(scope, this);

        let on_update = Self::get_by_name::<v8::Function>(scope, namespace, "OnUpdate");

        let this = v8::Global::new(scope, this);

        Ok(Self {
            delta_t,
            this,
            on_update,
        })
    }

    fn assign_delta_t(scope: &mut v8::HandleScope, obj: v8::Local<v8::Object>) -> Box<f64> {
        let mut delta_t = Box::new(0.);
        Self::set_external(scope, obj, 0, &mut delta_t);
        delta_t
    }

    fn get_delta_t(
        scope: &mut v8::HandleScope,
        _key: v8::Local<v8::Name>,
        args: v8::PropertyCallbackArguments,
        mut rv: v8::ReturnValue,
    ) {
        let this = args.this();
        let delta_t = Self::get_external::<f64>(scope, this, 0);
        rv.set_double(unsafe { *delta_t });
    }

    fn set_external<'a, T>(
        scope: &mut v8::HandleScope,
        obj: v8::Local<'a, v8::Object>,
        field_num: usize,
        value: &mut Box<T>,
    ) {
        let external = v8::External::new(scope, &mut **value as *mut T as *mut c_void);
        obj.set_internal_field(field_num, external.into());
    }

    fn get_external<'a, T>(
        scope: &mut v8::HandleScope,
        obj: v8::Local<'a, v8::Object>,
        field_num: usize,
    ) -> *mut T {
        let external = obj.get_internal_field(scope, field_num).unwrap();
        let external = unsafe { v8::Local::<v8::External>::cast(external) };
        external.value() as *mut T
    }

    fn get_by_name<'a, T>(
        scope: &mut HandleScope<'a>,
        namespace: &v8::Local<'a, v8::Object>,
        name: &str,
    ) -> Option<v8::Global<T>>
    where
        v8::Local<'a, T>: TryFrom<v8::Local<'a, deno_core::v8::Value>>,
    {
        let key = v8::String::new(scope, name)?;
        let value = namespace.get(scope, key.into())?;
        let value: v8::Local<T> = value.try_into().ok()?;
        Some(v8::Global::new(scope, value))
    }
}

#[derive(Debug)]
pub enum LogicInputEvent {
    Update(f64),
}

#[derive(Debug)]
pub enum LogicOutputEvent {
    Err(AnyError),
}

#[derive(Debug)]
struct InvocationInfo {
    binding_delay: Duration,
    function_invocation: Duration,
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

        let namespace = runtime.get_module_namespace(main_module)?;
        let handles = RuntimeHandles::new(&mut runtime, &namespace)?;

        Ok(Self {
            update_counter: 0,
            runtime,
            handles,
        })
    }

    pub async fn run_event_loop(&mut self) -> Result<(), AnyError> {
        self.runtime.run_event_loop(false).await?;
        Ok(())
    }

    pub fn on_update(
        &mut self,
        delta_t: f64,
    ) -> Result<Option<InvocationInfo>, deno_core::error::AnyError> {
        let start = Instant::now();
        let scope = &mut self.runtime.handle_scope();
        if let Some(on_update) = self.handles.on_update.as_ref() {
            let on_update = v8::Local::new(scope, on_update);
            let this = v8::Local::new(scope, &self.handles.this);

            *self.handles.delta_t.as_mut() = delta_t;

            let binding_done = Instant::now();
            let binding_delay = binding_done.duration_since(start);

            on_update.call(scope, this.into(), &[]);

            let function_invocation = Instant::now().duration_since(binding_done);
            self.update_counter += 1;

            Ok(Some(InvocationInfo {
                binding_delay,
                function_invocation,
            }))
        } else {
            Ok(None)
        }
    }

    pub async fn close(&mut self) {}
}
