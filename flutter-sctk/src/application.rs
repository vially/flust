use flutter_runner_api::ApplicationAttributes;
use thiserror::Error;

pub struct SctkApplication {}

impl SctkApplication {
    pub fn new(_attributes: ApplicationAttributes) -> Result<Self, SctkApplicationCreateError> {
        todo!()
    }

    pub fn run(self) -> Result<(), SctkApplicationRunError> {
        todo!()
    }
}

#[derive(Error, Debug)]
pub enum SctkApplicationCreateError {}

#[derive(Error, Debug)]
pub enum SctkApplicationRunError {}
