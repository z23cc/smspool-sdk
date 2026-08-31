use serde::Deserialize;

use crate::{
    Client, Error,
    api::wire,
    endpoint,
    types::{CountryId, DecimalValue, Money, PoolId, ServiceId},
};

#[derive(Clone, Debug)]
pub struct CatalogApi {
    client: Client,
}

impl CatalogApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn success_rates(&self, service: &ServiceId) -> Result<Vec<SuccessRate>, Error> {
        self.client
            .execute_endpoint(
                &endpoint::CATALOG_SUCCESS_RATE,
                wire([("service", service.to_string())]),
            )
            .await
    }

    pub async fn balance(&self) -> Result<Balance, Error> {
        self.client
            .execute_endpoint(&endpoint::CATALOG_BALANCE, wire([]))
            .await
    }

    pub async fn suggested_countries(
        &self,
        service: &ServiceId,
    ) -> Result<Vec<SuggestedCountry>, Error> {
        self.client
            .execute_endpoint(
                &endpoint::CATALOG_SUGGESTED_COUNTRIES,
                wire([("service", service.to_string())]),
            )
            .await
    }

    pub async fn suggested_pools(
        &self,
        service: &ServiceId,
        country: &CountryId,
        web: Option<bool>,
    ) -> Result<Vec<SuggestedPool>, Error> {
        let mut fields = vec![
            ("service", service.to_string()),
            ("country", country.to_string()),
        ];
        if let Some(web) = web {
            fields.push(("web", if web { "1" } else { "0" }.to_owned()));
        }
        self.client
            .execute_endpoint(&endpoint::CATALOG_SUGGESTED_POOLS, wire(fields))
            .await
    }

    /// The collection evidences a multipart body on this GET request; it is preserved verbatim.
    pub async fn countries(&self) -> Result<Vec<Country>, Error> {
        self.client
            .execute_endpoint(&endpoint::CATALOG_COUNTRIES, wire([]))
            .await
    }

    /// `country` is a disabled Postman field and is only sent when explicitly supplied.
    pub async fn services(&self, country: Option<&CountryId>) -> Result<Vec<Service>, Error> {
        let fields = country
            .map(|country| vec![("country", country.to_string())])
            .unwrap_or_default();
        self.client
            .execute_endpoint(&endpoint::CATALOG_SERVICES, wire(fields))
            .await
    }

    pub async fn pools(&self) -> Result<Vec<Pool>, Error> {
        self.client
            .execute_endpoint(&endpoint::CATALOG_POOLS, wire([]))
            .await
    }
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct SuccessRate {
    pub country: CountryId,
    pub country_id: CountryId,
    pub low_price: Money,
    pub name: String,
    pub price: Money,
    pub short_name: String,
    pub success_rate: DecimalValue,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct Balance {
    pub balance: Money,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct SuggestedCountry {
    pub country_id: CountryId,
    pub name: String,
    pub pool: PoolId,
    pub price: Money,
    pub short_name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct SuggestedPool {
    #[serde(deserialize_with = "crate::de::deserialize_lenient_bool")]
    pub custom_area: bool,
    pub name: String,
    pub pool: PoolId,
    pub price: Money,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct Country {
    #[serde(rename = "ID")]
    pub id: CountryId,
    pub name: String,
    pub region: String,
    pub short_name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct Service {
    #[serde(rename = "ID")]
    pub id: ServiceId,
    #[serde(deserialize_with = "crate::de::deserialize_lenient_bool")]
    pub favourite: bool,
    pub name: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct Pool {
    #[serde(rename = "ID")]
    pub id: PoolId,
    pub name: String,
}
