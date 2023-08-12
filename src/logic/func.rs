use deno_core::v8;

pub trait ExposedReturnable {
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