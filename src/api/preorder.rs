use serde::{Deserialize, Deserializer};

use crate::{
    Client, Error,
    api::wire,
    endpoint,
    types::{
        CountryId, Money, OrderId, PhoneNumber, PoolId, PreorderId, RawFormValue, ServiceId,
        StatusValue, VendorDateTime,
    },
};

use super::sms::ActionResponse;

#[derive(Clone, Debug)]
pub struct PreorderApi {
    client: Client,
}

impl PreorderApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn retrieve(&self) -> Result<Vec<Preorder>, Error> {
        self.client
            .execute_endpoint(&endpoint::PREORDER_RETRIEVE, wire([]))
            .await
    }

    pub async fn check(&self, order_id: &PreorderId) -> Result<PreorderStatus, Error> {
        self.client
            .execute_endpoint(
                &endpoint::PREORDER_CHECK,
                wire([("orderid", order_id.to_string())]),
            )
            .await
    }

    pub async fn cancel(&self, order_id: &PreorderId) -> Result<ActionResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::PREORDER_CANCEL,
                wire([("orderid", order_id.to_string())]),
            )
            .await
    }

    pub async fn create(&self, request: &CreatePreorderRequest) -> Result<PreorderCreated, Error> {
        self.client
            .execute_endpoint(&endpoint::PREORDER_CREATE, wire(request.fields()))
            .await
    }

    pub async fn price(&self, request: &PreorderPriceRequest) -> Result<PreorderPrice, Error> {
        self.client
            .execute_endpoint(&endpoint::PREORDER_PRICE, wire(request.fields()))
            .await
    }
}

#[derive(Clone, Debug)]
pub struct CreatePreorderRequest {
    service: ServiceId,
    country: CountryId,
    pool: PoolId,
    area_code: RawFormValue,
    max_price: Money,
}

impl CreatePreorderRequest {
    pub fn new(
        service: ServiceId,
        country: CountryId,
        pool: PoolId,
        area_code: RawFormValue,
        max_price: Money,
    ) -> Self {
        Self {
            service,
            country,
            pool,
            area_code,
            max_price,
        }
    }

    fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("service", self.service.to_string()),
            ("country", self.country.to_string()),
            ("pool", self.pool.to_string()),
            ("areacode", self.area_code.as_str().to_owned()),
            ("max_price", self.max_price.to_string()),
        ]
    }
}

#[derive(Clone, Debug)]
pub struct PreorderPriceRequest {
    service: ServiceId,
    country: CountryId,
    pool: PoolId,
}

impl PreorderPriceRequest {
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
pub struct Preorder {
    pub cost: Money,
    pub country: CountryId,
    pub country_name: String,
    pub highest_offer: Money,
    pub order_code: PreorderId,
    pub pool: PoolId,
    pub pool_name: String,
    pub service: ServiceId,
    pub service_name: String,
    pub status: String,
    pub time_left: u64,
    pub timestamp: VendorDateTime,
}

#[derive(Clone, Debug, Deserialize)]
struct PreorderStatusWire {
    cost: Money,
    country: CountryId,
    #[serde(default)]
    order_code: Option<OrderId>,
    #[serde(default)]
    phonenumber: Option<PhoneNumber>,
    pool: PoolId,
    preorder_code: PreorderId,
    service: ServiceId,
    status: StatusValue,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum PreorderStatus {
    Pending(PendingPreorder),
    Finished(FinishedPreorder),
}

impl<'de> Deserialize<'de> for PreorderStatus {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PreorderStatusWire::deserialize(deserializer)?;
        match (wire.order_code, wire.phonenumber) {
            (Some(order_code), Some(phonenumber)) => Ok(Self::Finished(FinishedPreorder {
                cost: wire.cost,
                country: wire.country,
                order_code,
                phonenumber,
                pool: wire.pool,
                preorder_code: wire.preorder_code,
                service: wire.service,
                status: wire.status,
            })),
            _ => Ok(Self::Pending(PendingPreorder {
                cost: wire.cost,
                country: wire.country,
                pool: wire.pool,
                preorder_code: wire.preorder_code,
                service: wire.service,
                status: wire.status,
            })),
        }
    }
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct PendingPreorder {
    pub cost: Money,
    pub country: CountryId,
    pub pool: PoolId,
    pub preorder_code: PreorderId,
    pub service: ServiceId,
    pub status: StatusValue,
}

#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct FinishedPreorder {
    pub cost: Money,
    pub country: CountryId,
    pub order_code: OrderId,
    pub phonenumber: PhoneNumber,
    pub pool: PoolId,
    pub preorder_code: PreorderId,
    pub service: ServiceId,
    pub status: StatusValue,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct PreorderCreated {
    pub expires_in: u64,
    pub message: String,
    pub order_code: PreorderId,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct PreorderPrice {
    pub cost: Money,
}
