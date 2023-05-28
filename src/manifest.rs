use crate::dom::{Dom, DomInst};
use crate::logic::runtime::{Logic, LogicInst};

#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Manifest {
    dom: Option<Dom>,
    logic: Option<Logic>,
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> serde_json::Result<Self> {
        serde_json::from_slice(bytes)
    }

    pub async fn spawn(&self) -> Instance {
      let dom = if let Some(dom) = self.dom.as_ref() { dom.spawn() } else { Default::default() };
      let logic = if let Some(logic) = self.logic.as_ref() { Some(logic.spawn().await.unwrap()) } else { None };
        Instance {
            dom,
            logic,
        }
    }
}

#[derive(Debug)]
pub struct Instance {
    pub dom: DomInst,
    pub logic: Option<LogicInst>,
}
