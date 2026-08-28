use serde::{de::Error as _, Deserialize, Deserializer};
use serde_json::Value;

use crate::{
    api::{invalid, wire},
    endpoint,
    types::{
        ActivationToken, Cents, CountryId, Money, OrderId, PhoneNumber, PoolId, RawFormValue,
        RedactedValue, ServiceId, SmsText, StatusValue, UnixTimestamp, VendorDateTime,
    },
    Client, Error,
};

#[derive(Clone, Debug)]
pub struct SmsApi {
    client: Client,
}

impl SmsApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn purchase(
        &self,
        request: &PurchaseSmsRequest,
    ) -> Result<PurchaseSmsResponse, Error> {
        self.client
            .execute_endpoint(&endpoint::SMS_PURCHASE, wire(request.fields(1, false)))
            .await
    }

    pub async fn check(&self, order_id: &OrderId) -> Result<SmsCheck, Error> {
        self.client
            .execute_endpoint(
                &endpoint::SMS_CHECK,
                wire([("orderid", order_id.to_string())]),
            )
            .await
    }

    pub async fn active(&self) -> Result<Vec<ActiveOrder>, Error> {
        self.client
            .execute_endpoint(&endpoint::SMS_ACTIVE, wire([]))
            .await
    }

    pub async fn cancel(&self, order_id: &OrderId) -> Result<ActionResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::SMS_CANCEL,
                wire([("orderid", order_id.to_string())]),
            )
            .await
    }

    pub async fn history(&self, request: &HistoryRequest) -> Result<Vec<OrderHistoryEntry>, Error> {
        self.client
            .execute_endpoint(&endpoint::SMS_HISTORY, wire(request.fields()))
            .await
    }
}

#[derive(Clone, Debug)]
pub struct ExperimentalSmsApi {
    client: Client,
}

impl ExperimentalSmsApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// Raw purchase for unverified quantity/token response modes.
    pub async fn purchase_raw(
        &self,
        request: &ExperimentalPurchaseRequest,
    ) -> Result<Value, Error> {
        self.client
            .execute_endpoint(
                &endpoint::SMS_PURCHASE,
                wire(request.base.fields(request.quantity, request.create_token)),
            )
            .await
    }

    pub async fn cancel_all(&self) -> Result<CancelAllResponse, Error> {
        self.client
            .execute_endpoint(&endpoint::SMS_CANCEL_ALL, wire([]))
            .await
    }

    pub async fn clear_cache(&self) -> Result<ActionResponse, Error> {
        self.client
            .execute_endpoint(&endpoint::SMS_CLEAR_CACHE, wire([]))
            .await
    }

    pub async fn activate(&self, order_id: &OrderId) -> Result<ActionResponse, Error> {
        self.order_action(&endpoint::SMS_ACTIVATE, order_id).await
    }

    pub async fn reactivate(&self, order_id: &OrderId) -> Result<ActionResponse, Error> {
        self.order_action(&endpoint::SMS_REACTIVATE, order_id).await
    }

    pub async fn archive(&self) -> Result<ActionResponse, Error> {
        self.client
            .execute_endpoint(&endpoint::SMS_ARCHIVE, wire([]))
            .await
    }

    pub async fn check_resend(&self, order_id: &OrderId) -> Result<ResendAvailability, Error> {
        self.client
            .execute_endpoint(
                &endpoint::SMS_CHECK_RESEND,
                wire([("orderid", order_id.to_string())]),
            )
            .await
    }

    pub async fn resend(&self, order_id: &OrderId) -> Result<ResendResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::SMS_RESEND,
                wire([("orderid", order_id.to_string())]),
            )
            .await
    }

    pub async fn stock(
        &self,
        country: &CountryId,
        service: &ServiceId,
        pool: &PoolId,
    ) -> Result<StockResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::SMS_STOCK,
                wire([
                    ("country", country.to_string()),
                    ("service", service.to_string()),
                    ("pool", pool.to_string()),
                ]),
            )
            .await
    }

    pub async fn all_stock(&self, filters: &AllStockFilters) -> Result<Vec<AllStockEntry>, Error> {
        self.client
            .execute_endpoint(&endpoint::SMS_ALL_STOCK, wire(filters.fields()))
            .await
    }

    /// No response example exists for this operation, so its result remains raw JSON.
    pub async fn area_codes(&self, request: &AreaCodesRequest) -> Result<Value, Error> {
        self.client
            .execute_endpoint(&endpoint::SMS_AREA_CODES, wire(request.fields()))
            .await
    }

    async fn order_action(
        &self,
        endpoint: &endpoint::Endpoint,
        order_id: &OrderId,
    ) -> Result<ActionResponse, Error> {
        self.client
            .execute_endpoint(endpoint, wire([("orderid", order_id.to_string())]))
            .await
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum PricingOption {
    Cheapest,
    HighestSuccessRate,
}

impl PricingOption {
    fn wire_value(self) -> &'static str {
        match self {
            Self::Cheapest => "0",
            Self::HighestSuccessRate => "1",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ActivationType {
    Sms,
    Voice,
    Flash,
}

impl ActivationType {
    fn wire_value(self) -> &'static str {
        match self {
            Self::Sms => "SMS",
            Self::Voice => "VOICE",
            Self::Flash => "FLASH",
        }
    }
}

#[derive(Clone, Debug)]
struct PurchaseFields {
    country: CountryId,
    service: ServiceId,
    pool: Option<PoolId>,
    max_price: Option<Money>,
    pricing_option: Option<PricingOption>,
    area_code: Option<RawFormValue>,
    exclude: Option<RawFormValue>,
    activation_type: ActivationType,
    carrier: Option<String>,
    phone_number: Option<PhoneNumber>,
}

impl PurchaseFields {
    fn new(country: CountryId, service: ServiceId) -> Self {
        Self {
            country,
            service,
            pool: None,
            max_price: None,
            pricing_option: None,
            area_code: None,
            exclude: None,
            activation_type: ActivationType::Sms,
            carrier: None,
            phone_number: None,
        }
    }

    fn fields(&self, quantity: u32, create_token: bool) -> Vec<(&'static str, String)> {
        let mut fields = vec![
            ("country", self.country.to_string()),
            ("service", self.service.to_string()),
            ("quantity", quantity.to_string()),
            (
                "create_token",
                if create_token { "1" } else { "0" }.to_owned(),
            ),
            (
                "activation_type",
                self.activation_type.wire_value().to_owned(),
            ),
        ];
        if let Some(value) = &self.pool {
            fields.push(("pool", value.to_string()));
        }
        if let Some(value) = &self.max_price {
            fields.push(("max_price", value.to_string()));
        }
        if let Some(value) = self.pricing_option {
            fields.push(("pricing_option", value.wire_value().to_owned()));
        }
        if let Some(value) = &self.area_code {
            fields.push(("areacode", value.as_str().to_owned()));
        }
        if let Some(value) = &self.exclude {
            fields.push(("exclude", value.as_str().to_owned()));
        }
        if let Some(value) = &self.carrier {
            fields.push(("carrier", value.clone()));
        }
        if let Some(value) = &self.phone_number {
            fields.push(("phonenumber", value.expose().to_owned()));
        }
        fields
    }
}

macro_rules! purchase_builders {
    ($ty:ty) => {
        impl $ty {
            pub fn pool(mut self, value: PoolId) -> Self {
                self.base.pool = Some(value);
                self
            }
            pub fn max_price(mut self, value: Money) -> Self {
                self.base.max_price = Some(value);
                self
            }
            pub fn pricing_option(mut self, value: PricingOption) -> Self {
                self.base.pricing_option = Some(value);
                self
            }
            pub fn area_code(mut self, value: RawFormValue) -> Self {
                self.base.area_code = Some(value);
                self
            }
            pub fn exclude(mut self, value: RawFormValue) -> Self {
                self.base.exclude = Some(value);
                self
            }
            pub fn activation_type(mut self, value: ActivationType) -> Self {
                self.base.activation_type = value;
                self
            }
            pub fn carrier(mut self, value: impl Into<String>) -> Result<Self, Error> {
                let value = value.into();
                if value.is_empty() {
                    return Err(invalid("carrier", "must not be empty"));
                }
                self.base.carrier = Some(value);
                Ok(self)
            }
            pub fn phone_number(mut self, value: PhoneNumber) -> Self {
                self.base.phone_number = Some(value);
                self
            }
        }
    };
}

#[derive(Clone, Debug)]
pub struct PurchaseSmsRequest {
    base: PurchaseFields,
}

impl PurchaseSmsRequest {
    pub fn new(country: CountryId, service: ServiceId) -> Self {
        Self {
            base: PurchaseFields::new(country, service),
        }
    }

    fn fields(&self, quantity: u32, create_token: bool) -> Vec<(&'static str, String)> {
        self.base.fields(quantity, create_token)
    }
}

purchase_builders!(PurchaseSmsRequest);

#[derive(Clone, Debug)]
pub struct ExperimentalPurchaseRequest {
    base: PurchaseFields,
    quantity: u32,
    create_token: bool,
}

impl ExperimentalPurchaseRequest {
    pub fn new(country: CountryId, service: ServiceId, quantity: u32) -> Result<Self, Error> {
        if quantity == 0 {
            return Err(invalid("quantity", "must be greater than zero"));
        }
        Ok(Self {
            base: PurchaseFields::new(country, service),
            quantity,
            create_token: false,
        })
    }

    pub fn create_token(mut self, value: bool) -> Self {
        self.create_token = value;
        self
    }
}

purchase_builders!(ExperimentalPurchaseRequest);

#[derive(Clone, Debug)]
pub struct HistoryRequest {
    start: u64,
    length: u64,
    search: String,
}

impl HistoryRequest {
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

    fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("start", self.start.to_string()),
            ("length", self.length.to_string()),
            ("search", self.search.clone()),
        ]
    }
}

#[derive(Clone, Debug, Default)]
pub struct AllStockFilters {
    country: Option<CountryId>,
    service: Option<ServiceId>,
    pool: Option<PoolId>,
}

impl AllStockFilters {
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
        fields
    }
}

#[derive(Clone, Debug)]
pub struct AreaCodesRequest {
    service: ServiceId,
    country: CountryId,
    pool: PoolId,
}

impl AreaCodesRequest {
    pub fn new(service: ServiceId, country: CountryId, pool: PoolId) -> Self {
        Self {
            service,
            country,
            pool,
        }
    }

    fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("service", self.service.to_string()),
            ("country", self.country.to_string()),
            ("pool", self.pool.to_string()),
        ]
    }
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct SmsOrder {
    pub cc: String,
    pub cost: Money,
    pub cost_in_cents: Cents,
    pub country: String,
    pub expiration: UnixTimestamp,
    pub expires_in: u64,
    pub message: String,
    pub number: PhoneNumber,
    pub order_id: OrderId,
    pub phonenumber: PhoneNumber,
    pub pool: PoolId,
    pub service: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct SmsToken {
    pub cc: String,
    pub message: String,
    pub phonenumber: PhoneNumber,
    pub token: ActivationToken,
    pub url: ActivationToken,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum PurchaseSmsResponse {
    Order(SmsOrder),
    Token(SmsToken),
}

impl<'de> Deserialize<'de> for PurchaseSmsResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        if value.get("order_id").is_some() {
            serde_json::from_value(value)
                .map(Self::Order)
                .map_err(D::Error::custom)
        } else if value.get("token").is_some() {
            serde_json::from_value(value)
                .map(Self::Token)
                .map_err(D::Error::custom)
        } else {
            Err(D::Error::custom("unknown purchase response shape"))
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
struct SmsCheckWire {
    #[serde(default)]
    expiration: Option<UnixTimestamp>,
    #[serde(default)]
    resend: Option<u64>,
    status: StatusValue,
    #[serde(default)]
    time_left: Option<u64>,
    #[serde(default)]
    full_sms: Option<SmsText>,
    #[serde(default)]
    sms: Option<SmsText>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum SmsCheck {
    Pending(PendingSms),
    Received(ReceivedSms),
    Terminated(TerminatedSms),
}

impl<'de> Deserialize<'de> for SmsCheck {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = SmsCheckWire::deserialize(deserializer)?;
        if wire.full_sms.is_some() || wire.sms.is_some() {
            Ok(Self::Received(ReceivedSms {
                expiration: wire.expiration,
                full_sms: wire.full_sms,
                sms: wire.sms,
                status: wire.status,
            }))
        } else if let Some(message) = wire.message {
            Ok(Self::Terminated(TerminatedSms {
                expiration: wire.expiration,
                message,
                resend: wire.resend,
                status: wire.status,
                time_left: wire.time_left,
            }))
        } else {
            Ok(Self::Pending(PendingSms {
                expiration: wire.expiration,
                resend: wire.resend,
                status: wire.status,
                time_left: wire.time_left,
            }))
        }
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PendingSms {
    pub expiration: Option<UnixTimestamp>,
    pub resend: Option<u64>,
    pub status: StatusValue,
    pub time_left: Option<u64>,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct ReceivedSms {
    pub expiration: Option<UnixTimestamp>,
    pub full_sms: Option<SmsText>,
    pub sms: Option<SmsText>,
    pub status: StatusValue,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct TerminatedSms {
    pub expiration: Option<UnixTimestamp>,
    pub message: String,
    pub resend: Option<u64>,
    pub status: StatusValue,
    pub time_left: Option<u64>,
}

fn optional_sms<'de, D>(deserializer: D) -> Result<Option<SmsText>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = String::deserialize(deserializer)?;
    if value.is_empty() {
        Ok(None)
    } else {
        SmsText::new(value).map(Some).map_err(D::Error::custom)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct ActiveOrder {
    pub code: SmsText,
    pub cost: Money,
    pub expiry: UnixTimestamp,
    #[serde(deserialize_with = "optional_sms")]
    pub full_code: Option<SmsText>,
    pub order_code: OrderId,
    pub phonenumber: PhoneNumber,
    pub service: String,
    pub short_name: String,
    pub status: String,
    pub time_left: u64,
    pub timestamp: VendorDateTime,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct ActionResponse {
    pub message: String,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct CancelAllResponse {
    pub message: String,
    pub refunded_orders: Vec<RedactedValue>,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct ResendAvailability {
    pub expires_in_hour: u64,
    pub message: String,
    #[serde(rename = "resendCost")]
    pub resend_cost: Money,
    pub resends: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct ResendResponse {
    pub charge: Money,
    pub message: String,
    pub order_id: OrderId,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct StockResponse {
    pub amount: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct AllStockEntry {
    pub country: CountryId,
    pub country_name: String,
    pub last_update: VendorDateTime,
    pub pool: PoolId,
    pub pool_name: String,
    pub price: Money,
    pub service: ServiceId,
    pub service_name: String,
    pub stock: u64,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct OrderHistoryEntry {
    pub code: SmsText,
    pub completed_on: VendorDateTime,
    pub cost: Money,
    pub expiry: UnixTimestamp,
    #[serde(deserialize_with = "optional_sms")]
    pub full_code: Option<SmsText>,
    pub order_code: OrderId,
    pub phonenumber: PhoneNumber,
    pub pool: PoolId,
    pub service: String,
    pub short_name: String,
    pub status: String,
    pub time_left: u64,
    pub timestamp: VendorDateTime,
}
