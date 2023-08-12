use std::ffi::c_void;
use std::marker::PhantomData;
use std::rc::Rc;
use std::time::Duration;
use std::time::Instant;

use deno_core::error::AnyError;
use deno_core::url::Url;
use deno_core::v8;
use deno_core::v8::HandleScope;
use deno_core::JsRuntime;
use deno_core::RuntimeOptions;

use super::func::ExposedReturnable;
use super::proxy::Logic;
use super::proxy::LogicInstEvent;

pub trait RuntimeExposed: Sized {
    fn setup_template(template: &mut ExposedTypeBinder<Self>);
}

pub fn create_runtime_template<'a, Exposed>(
    scope: &mut v8::HandleScope<'a>,
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

trait FieldAccessor {
    fn name() -> &'static str;
    fn num() -> usize;
    type Source;
    type Field: ExposedReturnable;
    fn get(source: &Self::Source, scope: &mut v8::HandleScope) -> Self::Field;
}

pub struct ExposedTypeBinder<'a, 'b, BoundType> {
    scope: &'b mut v8::HandleScope<'a>,
    template: &'b v8::Local<'a, v8::ObjectTemplate>,
    binding: RuntimeExposedBinding<BoundType>,
}

impl<'a, 'b, BoundType: RuntimeExposed> ExposedTypeBinder<'a, 'b, BoundType> {
    fn set_accessor<Getter>(&mut self)
    where
        Getter: FieldAccessor,
    {
        let scope = &mut self.scope;
        let name = v8::String::new(scope, Getter::name()).unwrap();

        let field_num = self.binding.field_num;
        self.template.set_accessor(
            name.into(),
            |scope: &mut v8::HandleScope,
             _key: v8::Local<v8::Name>,
             args: v8::PropertyCallbackArguments,
             rv: v8::ReturnValue| {
                let this = args.this();
                let local = Self::get_external(scope, &this, 1);
                let result = Getter::get(local, scope);
                result.set(rv);
            },
        );
    }

    fn accessor_impl<Getter: FieldAccessor>(
        scope: &mut v8::HandleScope,
        _key: v8::Local<v8::Name>,
        args: v8::PropertyCallbackArguments,
        rv: v8::ReturnValue,
    ) {
        let this = args.this();
        todo!()
        //let local = Self::get_external(scope, &this, Getter::num());
        //let result = Getter::get(local, scope);
        //result.set(rv);
    }

    fn get_external<'c, T>(
        scope: &mut v8::HandleScope,
        obj: &v8::Local<'c, v8::Object>,
        field_num: usize,
    ) -> &'c T {
        let external = obj.get_internal_field(scope, field_num).unwrap();
        let external = unsafe { v8::Local::<v8::External>::cast(external) };
        unsafe { &*(external.value() as *const T) }
    }
}

impl RuntimeExposed for RuntimeHandles {
    fn setup_template(template: &mut ExposedTypeBinder<Self>) {
        template.set_accessor::<DeltaTime>();
    }
}

struct DeltaTime;
impl FieldAccessor for DeltaTime {
    type Source = RuntimeHandles;
    type Field = f64;
    fn get(source: &Self::Source, _scope: &mut v8::HandleScope) -> Self::Field {
        *source.delta_t
    }

    fn name() -> &'static str {
        "deltaTime"
    }

    fn num() -> usize {
        0
    }
}

struct RuntimeHandles {
    delta_t: Box<f64>,
    this: v8::Global<v8::Object>,
    on_update: Option<v8::Global<v8::Function>>,
}

impl RuntimeHandles {
    fn new(
        runtime: &mut JsRuntime,
        namespace: &v8::Global<v8::Object>,
    ) -> Result<Self, AnyError> {
        let scope = &mut runtime.handle_scope();
        let namespace = &v8::Local::new(scope, namespace);

        let (this_template, accessor) = create_runtime_template::<RuntimeHandles>(scope);

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

    fn set_external<T>(
        scope: &mut v8::HandleScope,
        obj: v8::Local<v8::Object>,
        field_num: usize,
        value: &mut Box<T>,
    ) {
        let external = v8::External::new(scope, &mut **value as *mut T as *mut c_void);
        obj.set_internal_field(field_num, external.into());
    }

    fn get_external<T>(
        scope: &mut v8::HandleScope,
        obj: v8::Local<v8::Object>,
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

pub struct LogicRuntime {
    update_counter: u64,
    pub runtime: JsRuntime,
    handles: RuntimeHandles,
}

impl LogicRuntime {
    pub async fn process_message(&mut self, event: LogicInstEvent) -> Result<(), AnyError> {
        match event.input {
            LogicInputEvent::Update(delta_t) => {
                let start = Instant::now();
                let runtime = self.on_update(delta_t)?;
                println!(
                    "Message delay: {:?}, took {:#?}",
                    start.duration_since(event.send_time),
                    runtime
                );
            }
        }
        Ok(())
    }

    pub async fn spawn_runtime(
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
