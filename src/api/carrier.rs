use serde::Deserialize;

use crate::{Client, Error, api::wire, endpoint, types::PhoneNumber};

#[derive(Clone, Debug)]
pub struct CarrierApi {
    client: Client,
}

impl CarrierApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// This lookup is billed by the provider and is therefore never automatically retried.
    pub async fn paid_lookup(&self, phone: &PhoneNumber) -> Result<CarrierLookup, Error> {
        self.client
            .execute_endpoint(
                &endpoint::CARRIER_LOOKUP,
                wire([("phonenumber", phone.expose().to_owned())]),
            )
            .await
    }
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct CarrierLookup {
    pub carrier: String,
    pub carrier_type: String,
    pub country: String,
    pub phonenumber: PhoneNumber,
}
