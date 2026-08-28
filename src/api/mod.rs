//! Stable and explicitly experimental SMSPool resource APIs.

use serde_json::Value;

use crate::{Client, Error, TransportRequest};

pub mod business;
pub mod carrier;
pub mod catalog;
pub mod esim;
pub mod preorder;
pub mod pricing;
pub mod rental;
pub mod sms;
pub mod voucher;

/// Entry point for operations whose contracts are example-derived or incomplete.
#[derive(Clone, Debug)]
pub struct ExperimentalApi {
    client: Client,
}

impl ExperimentalApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    pub fn sms(&self) -> sms::ExperimentalSmsApi {
        sms::ExperimentalSmsApi::new(self.client.clone())
    }

    pub fn preorder(&self) -> preorder::PreorderApi {
        preorder::PreorderApi::new(self.client.clone())
    }

    pub fn rental(&self) -> rental::RentalApi {
        rental::RentalApi::new(self.client.clone())
    }

    pub fn carrier(&self) -> carrier::CarrierApi {
        carrier::CarrierApi::new(self.client.clone())
    }

    pub fn business(&self) -> business::BusinessApi {
        business::BusinessApi::new(self.client.clone())
    }

    pub fn esim(&self) -> esim::EsimApi {
        esim::EsimApi::new(self.client.clone())
    }

    pub fn voucher(&self) -> voucher::VoucherApi {
        voucher::VoucherApi::new(self.client.clone())
    }

    /// Execute an explicitly described raw request without bypassing transport policy.
    pub async fn raw(&self, request: TransportRequest) -> Result<Value, Error> {
        self.client.execute_json(request).await
    }
}

impl Client {
    pub fn catalog(&self) -> catalog::CatalogApi {
        catalog::CatalogApi::new(self.clone())
    }

    pub fn sms(&self) -> sms::SmsApi {
        sms::SmsApi::new(self.clone())
    }

    pub fn pricing(&self) -> pricing::PricingApi {
        pricing::PricingApi::new(self.clone())
    }

    pub fn experimental(&self) -> ExperimentalApi {
        ExperimentalApi::new(self.clone())
    }
}

pub(crate) fn wire(
    fields: impl IntoIterator<Item = (&'static str, String)>,
) -> crate::endpoint::WireRequest {
    crate::endpoint::WireRequest {
        body_fields: fields
            .into_iter()
            .map(|(name, value)| (name.to_owned(), value))
            .collect(),
        ..Default::default()
    }
}

pub(crate) fn invalid(field: &'static str, reason: &'static str) -> Error {
    Error::InvalidRequest { field, reason }
}
