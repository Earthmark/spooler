mod dom;
mod logic;
mod manifest;

#[derive(Debug)]
enum MainError {
    Serde(serde_json::Error),
    Tokio(tokio::task::JoinError),
    Deno(deno_core::error::AnyError),
}

impl From<serde_json::Error> for MainError {
    fn from(e: serde_json::Error) -> Self {
        Self::Serde(e)
    }
}

impl From<tokio::task::JoinError> for MainError {
    fn from(e: tokio::task::JoinError) -> Self {
        Self::Tokio(e)
    }
}

impl From<deno_core::error::AnyError> for MainError {
    fn from(e: deno_core::error::AnyError) -> Self {
        Self::Deno(e)
    }
}

#[tokio::main]
async fn main() {
    let manifest = manifest::Manifest::parse(include_bytes!("manifest.json")).unwrap();

    let mut instance = Box::new(manifest.spawn().await);

    println!("Manifest {:#?}", &manifest);
    println!("Instance {:#?}", &instance);

    if let Some(inst) = instance
        .logic
        .as_mut()
    {
        inst.on_update().unwrap();
    }

}
