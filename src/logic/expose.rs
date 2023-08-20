use std::ffi::c_void;
use std::marker::PhantomData;

use deno_core::v8::{self, HandleScope};

use super::interop::ExposedReturnable;

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
            _t: Default::default(),
        },
    };

    Exposed::setup_template(&mut binder);

    (template, binder.binding)
}

pub struct RuntimeExposedBinding<Type> {
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
        target.set_internal_field(0, external.into());
    }
}

pub struct ExposedTypeBinder<'a, 'b, BoundType> {
    scope: &'b mut v8::HandleScope<'a>,
    template: &'b v8::Local<'a, v8::ObjectTemplate>,
    binding: RuntimeExposedBinding<BoundType>,
}

trait FieldAccessor<BoundType> {
    type FieldResult: ExposedReturnable;
    fn name() -> &'static str;
    fn get(p: &BoundType, scope: &mut HandleScope) -> Self::FieldResult;
}

impl<BoundType: RuntimeExposed> ExposedTypeBinder<'_, '_, BoundType> {
    fn set_accessor<Getter>(&mut self)
    where
        Getter: FieldAccessor<BoundType>,
    {
        let scope = &mut self.scope;
        let name = v8::String::new(scope, Getter::name()).unwrap();

        self.template.set_accessor(
            name.into(),
            |scope: &mut v8::HandleScope,
             _key: v8::Local<v8::Name>,
             args: v8::PropertyCallbackArguments,
             rv: v8::ReturnValue| {
                let this = args.this();
                let local = Self::get_external(scope, &this, 0);
                let result = Getter::get(local, scope);
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

#[cfg(test)]
mod tests {
    use deno_core::{v8, JsRuntime, RuntimeOptions};

    use super::*;

    struct BoundObj {
        clock: i32,
    }

    struct BoundClockAccessor;

    impl FieldAccessor<BoundObj> for BoundClockAccessor {
        type FieldResult = i32;
        fn name() -> &'static str {
            "clock"
        }
        fn get(p: &BoundObj, _scope: &mut HandleScope) -> Self::FieldResult {
            p.clock
        }
    }

    impl RuntimeExposed for BoundObj {
        fn setup_template(template: &mut ExposedTypeBinder<Self>) {
            template.set_accessor::<BoundClockAccessor>();
        }
    }

    #[test]
    fn bind_trivial() {
        let runtime = &mut JsRuntime::new(RuntimeOptions::default());
        let mut obj = Box::new(BoundObj { clock: 21 });
        {
            let ctx = runtime.main_context();
            let scope = &mut runtime.handle_scope();

            let (template, installer) = create_runtime_template::<BoundObj>(scope);

            let ctx = v8::Local::new(scope, ctx);
            let global = ctx.global(scope);

            let js_target = template.new_instance(scope).unwrap();
            installer.set_internal_field(scope, &mut obj, js_target);

            let name = v8::String::new(scope, "test_target").unwrap();
            global.set(scope, name.into(), js_target.into()).unwrap();
        }
        let mut run_script = || {
            let value = runtime
                .execute_script_static("test_code", "test_target.clock")
                .unwrap();
            let scope = &mut runtime.handle_scope();
            let value = v8::Local::new(scope, value);
            value.int32_value(scope)
        };
        assert_eq!(run_script(), Some(21));
        obj.clock = 50;
        assert_eq!(run_script(), Some(50));
    }
}
