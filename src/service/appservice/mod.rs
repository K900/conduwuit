mod data;

pub(crate) use data::Data;
use ruma::api::appservice::Registration;

use crate::Result;

pub struct Service {
    pub db: &'static dyn Data,
}

impl Service {
    /// Registers an appservice and returns the ID to the caller
    pub fn register_appservice(&self, yaml: Registration) -> Result<String> {
        self.db.register_appservice(yaml)
    }

    /// Remove an appservice registration
    ///
    /// # Arguments
    ///
    /// * `service_name` - the name you send to register the service previously
    pub fn unregister_appservice(&self, service_name: &str) -> Result<()> {
        self.db.unregister_appservice(service_name)
    }

    pub fn get_registration(&self, id: &str) -> Result<Option<Registration>> {
        self.db.get_registration(id)
    }

    pub fn iter_ids(&self) -> Result<impl Iterator<Item = Result<String>> + '_> {
        self.db.iter_ids()
    }

    pub fn all(&self) -> Result<Vec<(String, Registration)>> {
        self.db.all()
    }
}
