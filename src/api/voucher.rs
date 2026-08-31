use serde_json::Value;

use crate::{
    Client, Error,
    api::{invalid, wire},
    endpoint,
    types::{ActivationToken, Money},
};

#[derive(Clone, Debug)]
pub struct VoucherApi {
    client: Client,
}

impl VoucherApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    /// No response example exists; the bounded JSON response is intentionally raw.
    pub async fn generate(&self, request: &GenerateVoucherRequest) -> Result<Value, Error> {
        self.client
            .execute_endpoint(
                &endpoint::VOUCHER_GENERATE,
                wire([("amount", request.amount.to_string())]),
            )
            .await
    }

    /// No response example exists; the bounded JSON response is intentionally raw.
    pub async fn retrieve(&self) -> Result<Value, Error> {
        self.client
            .execute_endpoint(&endpoint::VOUCHER_RETRIEVE, wire([]))
            .await
    }

    /// No response example exists; the bounded JSON response is intentionally raw.
    pub async fn delete(&self, voucher: &ActivationToken) -> Result<Value, Error> {
        self.client
            .execute_endpoint(
                &endpoint::VOUCHER_DELETE,
                wire([("voucher", voucher.expose().to_owned())]),
            )
            .await
    }

    /// Distinct from single generation despite sharing the provider path.
    pub async fn bulk_generate(
        &self,
        request: &BulkGenerateVouchersRequest,
    ) -> Result<Value, Error> {
        self.client
            .execute_endpoint(
                &endpoint::VOUCHER_BULK_GENERATE,
                wire([
                    ("amount", request.amount.to_string()),
                    ("quantity", request.quantity.to_string()),
                ]),
            )
            .await
    }
}

#[derive(Clone, Debug)]
pub struct GenerateVoucherRequest {
    amount: Money,
}

impl GenerateVoucherRequest {
    pub fn new(amount: Money) -> Self {
        Self { amount }
    }
}

#[derive(Clone, Debug)]
pub struct BulkGenerateVouchersRequest {
    amount: Money,
    quantity: u32,
}

impl BulkGenerateVouchersRequest {
    pub fn new(amount: Money, quantity: u32) -> Result<Self, Error> {
        if quantity == 0 {
            return Err(invalid("quantity", "must be greater than zero"));
        }
        Ok(Self { amount, quantity })
    }
}
