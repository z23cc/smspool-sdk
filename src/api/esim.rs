use serde::Deserialize;
use serde_json::Value;

use crate::{
    Client, Error,
    api::{invalid, wire},
    endpoint,
    types::{
        CountryId, DecimalValue, DecodedJson, EsimCredential, Money, PlanId, RedactedValue,
        StatusValue, TransactionId, VendorDateTime,
    },
};

use super::sms::ActionResponse;

#[derive(Clone, Debug)]
pub struct EsimApi {
    client: Client,
}

impl EsimApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn purchase(&self, plan: &PlanId) -> Result<EsimPurchaseResponse, Error> {
        self.client
            .execute_endpoint(&endpoint::ESIM_PURCHASE, wire([("plan", plan.to_string())]))
            .await
    }

    pub async fn pricing(&self, page: &EsimPageRequest) -> Result<EsimPricingPage, Error> {
        self.client
            .execute_endpoint(&endpoint::ESIM_PRICING, wire(page.fields("Search")))
            .await
    }

    pub async fn history(&self, page: &EsimPageRequest) -> Result<EsimHistoryPage, Error> {
        self.client
            .execute_endpoint(&endpoint::ESIM_HISTORY, wire(page.fields("search")))
            .await
    }

    pub async fn plans(&self, country: &CountryId) -> Result<Vec<EsimPlan>, Error> {
        self.client
            .execute_endpoint(
                &endpoint::ESIM_PLANS,
                wire([("country", country.to_string())]),
            )
            .await
    }

    pub async fn profile(&self, transaction: &TransactionId) -> Result<EsimProfile, Error> {
        self.transaction_call(&endpoint::ESIM_PROFILE, transaction)
            .await
    }

    pub async fn delete(&self, transaction: &TransactionId) -> Result<ActionResponse, Error> {
        self.transaction_call(&endpoint::ESIM_DELETE, transaction)
            .await
    }

    pub async fn top_up(
        &self,
        transaction: &TransactionId,
        plan: &PlanId,
    ) -> Result<ActionResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::ESIM_TOP_UP,
                wire([
                    ("transactionId", transaction.to_string()),
                    ("plan", plan.to_string()),
                ]),
            )
            .await
    }

    pub async fn top_up_plans(&self, plan: &PlanId) -> Result<Vec<EsimPlan>, Error> {
        self.client
            .execute_endpoint(
                &endpoint::ESIM_TOP_UP_PLANS,
                wire([("plan", plan.to_string())]),
            )
            .await
    }

    async fn transaction_call<T>(
        &self,
        endpoint: &endpoint::Endpoint,
        transaction: &TransactionId,
    ) -> Result<T, Error>
    where
        T: serde::de::DeserializeOwned,
    {
        self.client
            .execute_endpoint(endpoint, wire([("transactionId", transaction.to_string())]))
            .await
    }
}

#[derive(Clone, Debug)]
pub struct EsimPageRequest {
    start: u64,
    length: u64,
    search: String,
}

impl EsimPageRequest {
    pub fn new(start: u64, length: u64) -> Result<Self, Error> {
        if length == 0 {
            return Err(invalid("length", "must be greater than zero"));
        }
        Ok(Self {
            start,
            length,
            search: String::new(),
        })
    }

    pub fn search(mut self, value: impl Into<String>) -> Self {
        self.search = value.into();
        self
    }

    fn fields(&self, search_name: &'static str) -> Vec<(&'static str, String)> {
        vec![
            ("start", self.start.to_string()),
            ("length", self.length.to_string()),
            (search_name, self.search.clone()),
        ]
    }
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct EsimPurchaseResponse {
    pub message: String,
    #[serde(rename = "transactionId")]
    pub transaction_id: EsimCredential,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct EsimPricingPage {
    pub data: Vec<EsimPricingEntry>,
    pub rows: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct EsimPricingEntry {
    #[serde(rename = "ID")]
    pub id: PlanId,
    #[serde(rename = "countryCode")]
    pub country_code: String,
    #[serde(rename = "dataInGb")]
    pub data_in_gb: DecimalValue,
    pub extendable: StatusValue,
    pub name: String,
    pub network: DecodedJson<Value>,
    pub price: Money,
    pub speed: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct EsimHistoryPage {
    pub data: Vec<EsimHistoryEntry>,
    pub limit: u64,
    pub page: u64,
    pub rows: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct EsimHistoryEntry {
    pub cost: Money,
    #[serde(rename = "countryCode")]
    pub country_code: String,
    #[serde(rename = "dataInGb")]
    pub data_in_gb: DecimalValue,
    pub expiration: VendorDateTime,
    pub label: String,
    pub name: String,
    pub plan: PlanId,
    pub status: StatusValue,
    pub timestamp: VendorDateTime,
    #[serde(rename = "transactionId")]
    pub transaction_id: EsimCredential,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct EsimPlan {
    #[serde(rename = "ID")]
    pub id: PlanId,
    #[serde(rename = "dataInGb")]
    pub data_in_gb: DecimalValue,
    pub duration: u64,
    pub extendable: StatusValue,
    pub ip: String,
    pub network: DecodedJson<Value>,
    pub price: Money,
    pub speed: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct EsimProfile {
    pub ac: EsimCredential,
    pub activated: StatusValue,
    #[serde(rename = "activationCode")]
    pub activation_code: EsimCredential,
    pub apn: String,
    #[serde(rename = "countryCode")]
    pub country_code: String,
    pub label: Option<RedactedValue>,
    pub pin: EsimCredential,
    pub plan: PlanId,
    pub puk: EsimCredential,
    #[serde(rename = "remainingData")]
    pub remaining_data: String,
    pub smdp: EsimCredential,
    pub topup: StatusValue,
    #[serde(rename = "totalData")]
    pub total_data: String,
    #[serde(rename = "transactionId")]
    pub transaction_id: EsimCredential,
}
