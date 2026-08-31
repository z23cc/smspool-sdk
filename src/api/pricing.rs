use serde::Deserialize;

use crate::{
    api::wire,
    endpoint,
    types::{CountryId, Money, PoolId, ServiceId},
    Client, Error,
};

#[derive(Clone, Debug)]
pub struct PricingApi {
    client: Client,
}

impl PricingApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// Requires at least one filter.
    ///
    /// The unfiltered catalog was measured live at ~17.2 MiB, far above the client's response
    /// limit, so an unfiltered call could only ever fail with
    /// [`Error::ResponseTooLarge`]. Rejecting it locally turns that into an actionable error
    /// before anything is sent.
    ///
    /// Filtering is necessary but not automatically sufficient: `max_price=0.10` measured
    /// ~557 KiB and a single `country` ~327 KiB, both within the 1 MiB default but without much
    /// headroom. Scope queries as tightly as possible, or raise
    /// [`crate::ClientBuilder::max_response_bytes`] deliberately.
    pub async fn all(&self, filters: &PricingFilters) -> Result<Vec<PricingEntry>, Error> {
        let fields = filters.fields();
        if fields.is_empty() {
            return Err(Error::InvalidRequest {
                field: "pricing_filters",
                reason: "at least one filter is required; note that a broad filter can still exceed the response limit",
            });
        }
        self.client
            .execute_endpoint(&endpoint::PRICING_ALL, wire(fields))
            .await
    }

    pub async fn quote(
        &self,
        country: &CountryId,
        service: &ServiceId,
        pool: &PoolId,
    ) -> Result<PriceQuote, Error> {
        self.client
            .execute_endpoint(
                &endpoint::PRICING_QUOTE,
                wire([
                    ("country", country.to_string()),
                    ("service", service.to_string()),
                    ("pool", pool.to_string()),
                ]),
            )
            .await
    }
}

#[derive(Clone, Debug, Default)]
pub struct PricingFilters {
    country: Option<CountryId>,
    service: Option<ServiceId>,
    pool: Option<PoolId>,
    max_price: Option<Money>,
}

impl PricingFilters {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn country(mut self, value: CountryId) -> Self {
        self.country = Some(value);
        self
    }

    pub fn service(mut self, value: ServiceId) -> Self {
        self.service = Some(value);
        self
    }

    pub fn pool(mut self, value: PoolId) -> Self {
        self.pool = Some(value);
        self
    }

    pub fn max_price(mut self, value: Money) -> Self {
        self.max_price = Some(value);
        self
    }

    fn fields(&self) -> Vec<(&'static str, String)> {
        let mut fields = Vec::new();
        if let Some(value) = &self.country {
            fields.push(("country", value.to_string()));
        }
        if let Some(value) = &self.service {
            fields.push(("service", value.to_string()));
        }
        if let Some(value) = &self.pool {
            fields.push(("pool", value.to_string()));
        }
        if let Some(value) = &self.max_price {
            fields.push(("max_price", value.to_string()));
        }
        fields
    }
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct PricingEntry {
    pub country: CountryId,
    pub country_name: String,
    pub pool: PoolId,
    pub price: Money,
    pub service: ServiceId,
    pub service_name: String,
    pub short_name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct PriceQuote {
    pub high_price: Money,
    pub pool: PoolId,
    pub price: Money,
    pub success_rate: u64,
}
