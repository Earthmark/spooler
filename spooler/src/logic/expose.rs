use std::ffi::c_void;
use std::marker::PhantomData;

use deno_core::v8::{self, HandleScope};

use super::interop::ExposedReturnable;

pub trait RuntimeExposed: Sized {
    fn setup_template(template: &mut ExposedTypeBinder<Self>);
}

pub struct ProxyFactory<Type> {
    _t: PhantomData<Type>,
    template: v8::Global<v8::ObjectTemplate>,
}

impl<Type: RuntimeExposed> ProxyFactory<Type> {
    pub fn new<'a>(scope: &mut v8::HandleScope<'a>) -> Self {
        let template = v8::ObjectTemplate::new(scope);
        template.set_internal_field_count(1);

        let mut binder = ExposedTypeBinder {
            _t: Default::default(),
            scope,
            template: &template,
        };

        Type::setup_template(&mut binder);

        Self {
            _t: Default::default(),
            template: v8::Global::new(scope, template),
        }
    }

    pub fn proxy_pinned<'a>(
        &self,
        scope: &mut v8::HandleScope,
        obj: &mut Box<Type>,
    ) -> v8::Global<v8::Object> {
        let template = v8::Local::new(scope, &self.template);
        let instance = template.new_instance(scope).unwrap();

        Self::set_internal_field(obj, &instance);

        v8::Global::new(scope, instance)
    }

    fn set_internal_field(obj: &mut Box<Type>, target: &v8::Local<v8::Object>) {
        target.set_aligned_pointer_in_internal_field(0, &mut **obj as *mut Type as *mut c_void);
    }
}

pub struct ExposedTypeBinder<'a, 'b, BoundType> {
    _t: PhantomData<BoundType>,
    scope: &'b mut v8::HandleScope<'a>,
    template: &'b v8::Local<'a, v8::ObjectTemplate>,
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
                if let Some(local) = Self::get_external(&this, 0) {
                    let result = Getter::get(local, scope);
                    result.set(rv);
                }
            },
        );
    }

    fn get_external<'a, T>(obj: &v8::Local<'a, v8::Object>, field_num: i32) -> Option<&'a T> {
        let field = unsafe { obj.get_aligned_pointer_from_internal_field(field_num) as *const T };
        if field.is_null() {
            None
        } else {
            Some(unsafe { &*field })
        }
    }
}

#[cfg(test)]
mod tests {
    use deno_core::{v8, JsRuntime, RuntimeOptions};
    use spooler_macro::RuntimeExposed;

    use super::*;

    #[derive(RuntimeExposed)]
    struct BoundObj {
        clock: i32,
    }

    #[test]
    fn bind_trivial() {
        let runtime = &mut JsRuntime::new(RuntimeOptions::default());
        let mut obj = Box::new(BoundObj { clock: 21 });
        {
            let ctx = runtime.main_context();
            let scope = &mut runtime.handle_scope();

            let proxy_factory = ProxyFactory::<BoundObj>::new(scope);

            let ctx = v8::Local::new(scope, ctx);
            let global = ctx.global(scope);

            let proxy = proxy_factory.proxy_pinned(scope, &mut obj);
            let js_target = v8::Local::new(scope, proxy);

            let name = v8::String::new(scope, "test_target").unwrap();
            global.set(scope, name.into(), js_target.into()).unwrap();
        };
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
