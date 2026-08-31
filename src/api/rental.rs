use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{
    Client, Error,
    api::{invalid, wire},
    endpoint,
    types::{
        Days, DecimalValue, Money, PhoneNumber, RawFormValue, RedactedValue, RentalCode, RentalId,
        ServiceId, SmsText, UnixTimestamp, VendorDateTime,
    },
};

use super::sms::ActionResponse;

#[derive(Clone, Debug)]
pub struct RentalApi {
    client: Client,
}

impl RentalApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn catalog(
        &self,
        rental_type: &RawFormValue,
    ) -> Result<RentalCatalogResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::RENTAL_CATALOG,
                wire([("type", rental_type.as_str().to_owned())]),
            )
            .await
    }

    pub async fn purchase(
        &self,
        request: &RentalPurchaseRequest,
    ) -> Result<RentalPurchaseResponse, Error> {
        self.client
            .execute_endpoint(&endpoint::RENTAL_PURCHASE, wire(request.fields()))
            .await
    }

    pub async fn refund(&self, code: &RentalCode) -> Result<ActionResponse, Error> {
        self.code_action(&endpoint::RENTAL_REFUND, code).await
    }

    pub async fn extend(
        &self,
        code: &RentalCode,
        days: Days,
    ) -> Result<RentalExtendResponse, Error> {
        if days.value() == 0 {
            return Err(invalid("days", "must be greater than zero"));
        }
        self.client
            .execute_endpoint(
                &endpoint::RENTAL_EXTEND,
                wire([
                    ("days", days.value().to_string()),
                    ("rental_code", code.to_string()),
                ]),
            )
            .await
    }

    pub async fn set_auto_extend(&self, code: &RentalCode) -> Result<AutoExtendResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::RENTAL_AUTO_EXTEND,
                wire([("rental_code", code.to_string())]),
            )
            .await
    }

    pub async fn history(&self) -> Result<Vec<RentalHistoryEntry>, Error> {
        self.client
            .execute_endpoint(&endpoint::RENTAL_HISTORY, wire([]))
            .await
    }

    pub async fn messages(&self, code: &RentalCode) -> Result<RentalMessagesResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::RENTAL_MESSAGES,
                wire([("rental_code", code.to_string())]),
            )
            .await
    }

    pub async fn status(&self, code: &RentalCode) -> Result<RentalStatusResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::RENTAL_STATUS,
                wire([("rental_code", code.to_string())]),
            )
            .await
    }

    pub async fn reset(&self, code: &RentalCode) -> Result<ActionResponse, Error> {
        self.code_action(&endpoint::RENTAL_RESET, code).await
    }

    pub async fn services(&self, rental: &RentalId) -> Result<Vec<RentalService>, Error> {
        self.client
            .execute_endpoint(
                &endpoint::RENTAL_SERVICES,
                wire([("rental", rental.to_string())]),
            )
            .await
    }

    pub async fn active(&self) -> Result<Vec<ActiveRental>, Error> {
        self.client
            .execute_endpoint(&endpoint::RENTAL_ACTIVE, wire([]))
            .await
    }

    pub async fn pricing(&self, id: &RentalId) -> Result<RentalPricingResponse, Error> {
        self.client
            .execute_endpoint(&endpoint::RENTAL_PRICING, wire([("id", id.to_string())]))
            .await
    }

    pub async fn stock(&self, id: &RentalId, days: Days) -> Result<RentalStockResponse, Error> {
        if days.value() == 0 {
            return Err(invalid("days", "must be greater than zero"));
        }
        self.client
            .execute_endpoint(
                &endpoint::RENTAL_STOCK,
                wire([("id", id.to_string()), ("days", days.value().to_string())]),
            )
            .await
    }

    pub async fn info(&self, code: &RentalCode) -> Result<RentalInfo, Error> {
        self.client
            .execute_endpoint(
                &endpoint::RENTAL_INFO,
                wire([("rental_code", code.to_string())]),
            )
            .await
    }

    async fn code_action(
        &self,
        endpoint: &endpoint::Endpoint,
        code: &RentalCode,
    ) -> Result<ActionResponse, Error> {
        self.client
            .execute_endpoint(endpoint, wire([("rental_code", code.to_string())]))
            .await
    }
}

#[derive(Clone, Debug)]
pub struct RentalPurchaseRequest {
    id: RentalId,
    days: Days,
    service_id: ServiceId,
    create_token: bool,
}

impl RentalPurchaseRequest {
    pub fn new(id: RentalId, days: Days, service_id: ServiceId) -> Result<Self, Error> {
        if days.value() == 0 {
            return Err(invalid("days", "must be greater than zero"));
        }
        Ok(Self {
            id,
            days,
            service_id,
            create_token: false,
        })
    }

    /// Token-mode responses are not evidenced; enabling this remains experimental.
    pub fn create_token(mut self, value: bool) -> Self {
        self.create_token = value;
        self
    }

    fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("id", self.id.to_string()),
            ("days", self.days.value().to_string()),
            ("service_id", self.service_id.to_string()),
            (
                "create_token",
                if self.create_token { "1" } else { "0" }.to_owned(),
            ),
        ]
    }
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalCatalogResponse {
    pub data: Vec<RentalCatalogItem>,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalCatalogItem {
    #[serde(rename = "ID")]
    pub id: RentalId,
    #[serde(deserialize_with = "crate::de::deserialize_lenient_bool")]
    pub is_refundable: bool,
    pub name: String,
    pub pool: u64,
    pub pricing: BTreeMap<String, DecimalValue>,
    pub priority: u64,
    pub refund_min_days: u64,
    pub refund_within: u64,
    pub region: String,
    pub single_service: Option<RedactedValue>,
    pub single_service_extend: Option<RedactedValue>,
    pub tag: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalPurchaseResponse {
    pub days: u64,
    pub expiry: UnixTimestamp,
    pub message: String,
    pub phonenumber: PhoneNumber,
    pub rental_code: RentalCode,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalExtendResponse {
    pub expiration_date: UnixTimestamp,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct AutoExtendResponse {
    #[serde(deserialize_with = "crate::de::deserialize_lenient_bool")]
    pub auto_extend: bool,
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalHistoryEntry {
    #[serde(rename = "ID")]
    pub id: u64,
    pub action: String,
    pub cost: Money,
    pub rental: RentalCode,
    pub timestamp: VendorDateTime,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalMessagesResponse {
    pub messages: Vec<RentalMessage>,
    pub source: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalMessage {
    #[serde(rename = "ID")]
    pub id: u64,
    pub message: SmsText,
    pub sender: PhoneNumber,
    pub timestamp: VendorDateTime,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalStatusResponse {
    pub status: RentalStatus,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalStatus {
    #[serde(rename = "activeFor")]
    pub active_for: u64,
    #[serde(deserialize_with = "crate::de::deserialize_lenient_bool")]
    pub auto_extend: bool,
    #[serde(deserialize_with = "crate::de::deserialize_lenient_bool")]
    pub available: bool,
    pub expiry: UnixTimestamp,
    pub phonenumber: PhoneNumber,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalService {
    #[serde(rename = "ID")]
    pub id: ServiceId,
    pub name: String,
    pub pool: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct ActiveRental {
    pub country_name: String,
    pub expiration_date: UnixTimestamp,
    pub phonenumber: PhoneNumber,
    pub rental: RentalId,
    pub rental_code: RentalCode,
    pub source: u64,
    pub state: String,
    #[serde(rename = "type")]
    pub rental_type: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalPricingResponse {
    pub extend: BTreeMap<String, DecimalValue>,
    pub pricing: BTreeMap<String, DecimalValue>,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalStockResponse {
    pub count: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct RentalInfo {
    #[serde(deserialize_with = "crate::de::deserialize_lenient_bool")]
    pub auto_extend: bool,
    pub country_name: String,
    pub expiration_date: UnixTimestamp,
    pub phonenumber: PhoneNumber,
    pub price: Money,
    #[serde(deserialize_with = "crate::de::deserialize_lenient_bool")]
    pub refund: bool,
    pub rental: RentalId,
    pub rental_code: RentalCode,
    pub service: ServiceId,
    pub service_name: String,
    pub source: u64,
    #[serde(rename = "type")]
    pub rental_type: u64,
}
