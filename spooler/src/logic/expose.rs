use std::any::TypeId;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::ffi::c_void;
use std::marker::PhantomData;

use deno_core::v8::{self, HandleScope};

use super::interop::ExposedReturnable;

#[derive(Default)]
pub struct ProxyFactory {
    map: HashMap<TypeId, v8::Global<v8::ObjectTemplate>>,
}

impl ProxyFactory {
    pub fn proxy<'a, Type>(&mut self, scope: &mut HandleScope, val: &'a mut Box<Type>) -> TypeProxy<'a, Type>
    where
        Type: RuntimeExposed + 'static,
    {
        let entry = self.map.entry(TypeId::of::<Type>());

        let template = match &entry {
            Entry::Occupied(v) => v8::Local::new(scope, v.get()),
            Entry::Vacant(_) => Self::create_proxy::<Type>(scope),
        };

        let proxy = TypeProxy::new_from_template(scope, template, val);

        entry.or_insert_with(|| v8::Global::new(scope, template));

        proxy
    }

    fn create_proxy<'a, Type>(scope: &mut HandleScope<'a>) -> v8::Local<'a, v8::ObjectTemplate>
    where
        Type: RuntimeExposed,
    {
        let template = v8::ObjectTemplate::new(scope);
        template.set_internal_field_count(1);

        let mut binder = ExposedTypeBinder {
            _t: Default::default(),
            scope,
            template: &template,
        };

        Type::setup_template(&mut binder);

        template
    }
}

pub trait RuntimeExposed: Sized {
    fn setup_template(template: &mut ExposedTypeBinder<Self>);
}

pub struct TypeProxy<'a, Type> {
    src: &'a Box<Type>,
    proxy: v8::Global<v8::Object>,
}

impl<'a, Type> TypeProxy<'a, Type> {
    fn new_from_template(
        scope: &mut v8::HandleScope,
        proxy_template: v8::Local<v8::ObjectTemplate>,
        src: &'a mut Box<Type>,
    ) -> Self {
        let instance = proxy_template.new_instance(scope).unwrap();
        Self::set_internal_field(&instance, src);
        let proxy = v8::Global::new(scope, instance);
        TypeProxy { src, proxy }
    }

    fn set_internal_field(target: &v8::Local<v8::Object>, obj: &mut Box<Type>) {
        target.set_aligned_pointer_in_internal_field(0, &mut **obj as *mut Type as *mut c_void);
    }

    fn clear_internal_field(target: &v8::Local<v8::Object>) {
        target.set_aligned_pointer_in_internal_field(0, std::ptr::null_mut());
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

            let mut proxy_factory = ProxyFactory::default();

            let ctx = v8::Local::new(scope, ctx);
            let global = ctx.global(scope);

            let proxy = proxy_factory.proxy(scope, &mut obj);
            let js_target = v8::Local::new(scope, proxy.proxy);

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
