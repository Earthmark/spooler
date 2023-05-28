use deno_core::v8::{Function, Global, Local, Object, Script, String};
use deno_core::{op, v8, Extension, JsRuntime, ModuleLoader, ModuleSpecifier, RuntimeOptions};

use std::pin::Pin;

use criterion::{criterion_group, criterion_main, Criterion};

#[op]
fn op_sum(nums: Vec<f64>) -> f64 {
    let sum = nums.iter().fold(0.0, |a, v| a + v);
    sum
}

fn make_runtime() -> JsRuntime {
    JsRuntime::new(RuntimeOptions {
        extensions: vec![Extension::builder().ops(vec![op_sum::decl()]).build()],
        module_loader: Some(std::rc::Rc::new(ModsLoader)),
        ..Default::default()
    })
}

struct ModsLoader;

impl ModuleLoader for ModsLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _is_main: bool,
    ) -> Result<ModuleSpecifier, bevy::asset::Error> {
        assert_eq!(specifier, "file:///main.js");
        assert_eq!(referrer, ".");
        let s = deno_core::resolve_import(specifier, referrer).unwrap();
        Ok(s)
    }

    fn load(
        &self,
        _module_specifier: &ModuleSpecifier,
        _maybe_referrer: Option<ModuleSpecifier>,
        _is_dyn_import: bool,
    ) -> Pin<Box<deno_core::ModuleSourceFuture>> {
        todo!()
    }
}

fn load_module(runtime: &mut JsRuntime, code: &str) -> v8::Global<v8::Object> {
    let module_id = deno_core::futures::executor::block_on(runtime.load_main_module(
        &deno_core::resolve_url("file:///main.js").unwrap(),
        Some(code.into()),
    ))
    .unwrap();

    _ = runtime.mod_evaluate(module_id);

    runtime.get_module_namespace(module_id).unwrap()
}

fn compile_script(runtime: &mut JsRuntime, script: &str) -> Global<Script> {
    let scope = &mut runtime.handle_scope();
    let source = String::new(scope, script).unwrap();
    let script = Script::compile(scope, source, None).unwrap();
    Global::new(scope, script)
}

fn get_module_function(
    runtime: &mut JsRuntime,
    module_ns: &Global<Object>,
    name: &str,
) -> Global<Function> {
    let scope = &mut runtime.handle_scope();

    let module_ns = Local::new(scope, module_ns);

    assert!(module_ns.is_module_namespace_object());

    let func_name = String::new(scope, name).unwrap();
    let binding = module_ns.get(scope, func_name.into()).unwrap();

    assert!(binding.is_function());

    let binding: Local<Function> = binding.try_into().unwrap();

    Global::new(scope, binding)
}

fn create_and_execute_runtime(c: &mut Criterion) {
    c.bench_function("Runtime creation and execution", |b| {
        b.iter(|| {
            let mut runtime = make_runtime();
            runtime
                .execute_script("<usage>", r#"Deno.core.ops.op_sum([1, 2, 3]);"#)
                .unwrap()
        })
    });
}

fn execute_script_over_runtime(c: &mut Criterion) {
    let mut runtime = make_runtime();

    c.bench_function("execute array create script", |b| {
        b.iter(|| runtime.execute_script("<usage>", r#"[1, 2, 3];"#).unwrap())
    });

    c.bench_function("execute two array create script", |b| {
        b.iter(|| {
            runtime
                .execute_script("<usage>", r#"[1, 2, 3];[1, 2, 3];"#)
                .unwrap()
        })
    });

    c.bench_function("Deno native function invoke", |b| {
        b.iter(|| {
            runtime
                .execute_script("<usage>", r#"Deno.core.ops.op_sum([1, 2, 3]);"#)
                .unwrap()
        })
    });
}

fn pre_compile_script(c: &mut Criterion) {
    let mut runtime = make_runtime();

    let script = compile_script(&mut runtime, r#"Deno.core.ops.op_sum([1, 2, 3]);"#);

    c.bench_function("execute cached script", |b| {
        b.iter(|| {
            let scope = &mut runtime.handle_scope();
            let script = Local::new(scope, &script);
            script.run(scope).unwrap();
        })
    });

    c.bench_function("execute cached script in cached handle scope", |b| {
        let scope = &mut runtime.handle_scope();
        let script = Local::new(scope, &script);
        b.iter(|| {
            script.run(scope).unwrap();
        })
    });
}

fn invoke_main_module(c: &mut Criterion) {
    let mut runtime = make_runtime();

    let module_ns = load_module(&mut runtime, include_str!("js_mod.js"));

    c.bench_function("Invoke module defined function", |b| {
        let func = get_module_function(&mut runtime, &module_ns, "TestFunc");
        let scope = &mut runtime.handle_scope();
        let f = Local::new(scope, &func);
        let module_ns = Local::new(scope, &module_ns);
        b.iter(|| {
            f.call(scope, module_ns.into(), &[]).unwrap();
        })
    });

    c.bench_function("Invoke module defined lambda function", |b| {
        let func = get_module_function(&mut runtime, &module_ns, "LambdaFunc");
        let scope = &mut runtime.handle_scope();
        let f = Local::new(scope, &func);
        let module_ns = Local::new(scope, &module_ns);
        b.iter(|| {
            f.call(scope, module_ns.into(), &[]).unwrap();
        })
    });

    c.bench_function(
        "Invoke module defined state mutating lambda function",
        |b| {
            let func = get_module_function(&mut runtime, &module_ns, "IncrementCounter");
            let scope = &mut runtime.handle_scope();
            let f = Local::new(scope, &func);
            let module_ns = Local::new(scope, &module_ns);
            b.iter(|| {
                f.call(scope, module_ns.into(), &[]).unwrap();
            })
        },
    );
}

criterion_group!(
    runtime_checks,
    create_and_execute_runtime,
    execute_script_over_runtime,
    pre_compile_script,
    invoke_main_module
);
criterion_main!(runtime_checks);
