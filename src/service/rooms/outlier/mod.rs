mod data;
pub use data::Data;

use crate::service::*;

pub struct Service<D: Data> {
    db: D,
}

impl Service<_> {
    /// Returns the pdu from the outlier tree.
    pub fn get_outlier_pdu_json(&self, event_id: &EventId) -> Result<Option<CanonicalJsonObject>> {
        self.db.get_outlier_pdu_json(event_id)
    }

    /// Returns the pdu from the outlier tree.
    pub fn get_pdu_outlier(&self, event_id: &EventId) -> Result<Option<PduEvent>> {
        self.db.get_outlier_pdu(event_id)
    }

    /// Append the PDU as an outlier.
    #[tracing::instrument(skip(self, pdu))]
    pub fn add_pdu_outlier(&self, event_id: &EventId, pdu: &CanonicalJsonObject) -> Result<()> {
        self.db.add_pdu_outlier(event_id, pdu)
    }
}
