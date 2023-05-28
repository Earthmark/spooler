#[derive(serde::Deserialize, serde::Serialize, Debug)]
pub struct Dom {}

impl Dom {
  pub fn spawn(&self) -> DomInst {
    DomInst {
      }
  }
}

#[derive(Default, Debug)]
pub struct DomInst {

}
