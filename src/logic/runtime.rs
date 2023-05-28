use std::collections::BTreeMap;
use std::pin::Pin;
use std::rc::Rc;

use deno_core::futures::FutureExt;
use deno_core::url::Url;
use deno_core::v8;
use deno_core::JsRuntime;
use deno_core::ModuleLoader;
use deno_core::ModuleSource;
use deno_core::RuntimeOptions;

#[derive(serde::Deserialize, serde::Serialize, Debug, Clone)]
pub struct Logic {
    modules: BTreeMap<Url, String>,
}

impl Logic {
    pub async fn spawn(&self) -> Result<LogicInst, deno_core::error::AnyError> {
        LogicInst::new(&self, &Url::parse("local://main").unwrap()).await
    }
}

pub struct LogicInst {
    runtime: JsRuntime,
    main_module: i32,
    this: v8::Global<v8::Object>,
    on_update: Option<v8::Global<v8::Function>>,
}

impl std::fmt::Debug for LogicInst {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogicInst").finish()
    }
}

impl LogicInst {
    async fn new(module_loader: &Logic, url: &Url) -> Result<Self, deno_core::error::AnyError> {
        let mut runtime = JsRuntime::new(RuntimeOptions {
            module_loader: Some(Rc::new(module_loader.clone())),
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
            main_module,
            on_update,
            this,
        })
    }

    pub fn on_update(&mut self) -> Result<(), deno_core::error::AnyError> {
        let scope = &mut self.runtime.handle_scope();
        if let Some(on_update) = self.on_update.as_ref() {
            let on_update = v8::Local::new(scope, on_update);
            let this = v8::Local::new(scope, &self.this);
            on_update.call(scope, this.into(), &[]);
        }

        Ok(())
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
                    code: code.as_bytes().to_vec().into_boxed_slice(),
                    module_type: deno_core::ModuleType::JavaScript,
                    module_url_specified: module_specifier.to_string(),
                    module_url_found: module_specifier.to_string(),
                })
        }
        .boxed_local()
    }
}
