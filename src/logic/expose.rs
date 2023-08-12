use std::ffi::c_void;
use std::marker::PhantomData;

use deno_core::v8;

use super::func::ExposedReturnable;

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

pub struct ExposedTypeBinder<'a, 'b, BoundType> {
  scope: &'b mut v8::HandleScope<'a>,
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