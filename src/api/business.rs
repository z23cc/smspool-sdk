use std::collections::BTreeMap;

use serde::Deserialize;

use crate::{
    Client, Error,
    api::{invalid, wire},
    endpoint,
    types::{ActivationToken, BusinessUserId, Money, Password, RedactedValue},
};

use super::sms::ActionResponse;

#[derive(Clone, Debug)]
pub struct BusinessApi {
    client: Client,
}

impl BusinessApi {
    pub(crate) fn new(client: Client) -> Self {
        Self { client }
    }

    pub async fn update_user(
        &self,
        request: &BusinessUpdateRequest,
    ) -> Result<BusinessUpdateResponse, Error> {
        self.client
            .execute_endpoint(&endpoint::BUSINESS_UPDATE_USER, wire(request.fields()?))
            .await
    }

    pub async fn user_history(
        &self,
        id: &BusinessUserId,
    ) -> Result<BusinessHistoryResponse, Error> {
        self.client
            .execute_endpoint(
                &endpoint::BUSINESS_USER_HISTORY,
                wire([("id", id.to_string())]),
            )
            .await
    }

    pub async fn users(&self) -> Result<Vec<BusinessUser>, Error> {
        self.client
            .execute_endpoint(&endpoint::BUSINESS_USERS, wire([]))
            .await
    }

    pub async fn create_user(
        &self,
        request: &CreateBusinessUserRequest,
    ) -> Result<ActionResponse, Error> {
        self.client
            .execute_endpoint(&endpoint::BUSINESS_CREATE_USER, wire(request.fields()))
            .await
    }
}

#[derive(Clone, Debug)]
pub struct BusinessUpdateRequest {
    id: BusinessUserId,
    password: Option<Password>,
    balance: Option<Money>,
    active: Option<bool>,
}

impl BusinessUpdateRequest {
    pub fn new(id: BusinessUserId) -> Self {
        Self {
            id,
            password: None,
            balance: None,
            active: None,
        }
    }

    pub fn password(mut self, value: Password) -> Self {
        self.password = Some(value);
        self
    }

    pub fn balance(mut self, value: Money) -> Self {
        self.balance = Some(value);
        self
    }

    pub fn active(mut self, value: bool) -> Self {
        self.active = Some(value);
        self
    }

    fn fields(&self) -> Result<Vec<(&'static str, String)>, Error> {
        if self.password.is_none() && self.balance.is_none() && self.active.is_none() {
            return Err(invalid(
                "business_update",
                "at least one of password, balance, or active is required",
            ));
        }
        let mut fields = vec![("id", self.id.to_string())];
        if let Some(value) = &self.password {
            fields.push(("password", value.expose().to_owned()));
        }
        if let Some(value) = &self.balance {
            fields.push(("balance", value.to_string()));
        }
        if let Some(value) = self.active {
            fields.push(("active", if value { "1" } else { "0" }.to_owned()));
        }
        Ok(fields)
    }
}

#[derive(Clone, Debug)]
pub struct CreateBusinessUserRequest {
    username: String,
    password: Password,
}

impl CreateBusinessUserRequest {
    pub fn new(username: impl Into<String>, password: Password) -> Result<Self, Error> {
        let username = username.into();
        if username.is_empty() {
            return Err(invalid("username", "must not be empty"));
        }
        Ok(Self { username, password })
    }

    fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("username", self.username.clone()),
            ("password", self.password.expose().to_owned()),
        ]
    }
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct BusinessUpdateResponse {
    pub query: BTreeMap<String, ActionResponse>,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct BusinessHistoryResponse {
    pub history: Vec<RedactedValue>,
}

#[derive(Clone, Debug, Deserialize)]
#[non_exhaustive]
pub struct BusinessUser {
    #[serde(rename = "ID")]
    pub id: BusinessUserId,
    #[serde(deserialize_with = "crate::de::deserialize_lenient_bool")]
    pub active: bool,
    pub apikey: ActivationToken,
    pub balance: Money,
    pub username: String,
}
