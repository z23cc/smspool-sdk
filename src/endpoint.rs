use std::fmt;

use http::Method;
use serde_json::Value;

/// Immutable wire-level metadata for one vendor operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Endpoint {
    pub(crate) name: &'static str,
    pub(crate) method: Method,
    pub(crate) path: &'static str,
    pub(crate) body_mode: BodyMode,
    pub(crate) auth: AuthMode,
    pub(crate) safety: SafetyClass,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BodyMode {
    None,
    Multipart,
    FormUrlEncoded,
    RawJson,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AuthMode {
    Bearer,
    FormKey,
    BearerAndFormKey,
    Public,
}

/// Retry and ambiguity policy for an operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SafetyClass {
    ReadOnly,
    Mutation,
    PaidMutation,
}

/// An encoded request whose values must never appear in default diagnostics.
#[derive(Default)]
pub(crate) struct WireRequest {
    pub(crate) body_mode: Option<BodyMode>,
    pub(crate) body_fields: Vec<(String, String)>,
    pub(crate) query_fields: Vec<(String, String)>,
    pub(crate) raw_json: Option<Value>,
}

impl fmt::Debug for WireRequest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WireRequest")
            .field("body_mode", &self.body_mode)
            .field("body_field_count", &self.body_fields.len())
            .field("query_field_count", &self.query_fields.len())
            .field("has_raw_json", &self.raw_json.is_some())
            .finish()
    }
}

macro_rules! endpoint {
    ($constant:ident, $name:literal, $method:ident, $path:literal, $body:ident, $auth:ident, $safety:ident) => {
        pub(crate) static $constant: Endpoint = Endpoint {
            name: $name,
            method: Method::$method,
            path: $path,
            body_mode: BodyMode::$body,
            auth: AuthMode::$auth,
            safety: SafetyClass::$safety,
        };
    };
}

endpoint!(
    CATALOG_SUCCESS_RATE,
    "catalog.success_rate",
    POST,
    "/request/success_rate",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    CATALOG_BALANCE,
    "catalog.balance",
    POST,
    "/request/balance",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    CATALOG_SUGGESTED_COUNTRIES,
    "catalog.suggested_countries",
    POST,
    "/request/suggested_countries",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    CATALOG_SUGGESTED_POOLS,
    "catalog.suggested_pools",
    POST,
    "/pool/retrieve_valid",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    CATALOG_COUNTRIES,
    "catalog.countries",
    GET,
    "/country/retrieve_all",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    CATALOG_SERVICES,
    "catalog.services",
    GET,
    "/service/retrieve_all",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    CATALOG_POOLS,
    "catalog.pools",
    POST,
    "/pool/retrieve_all",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    SMS_PURCHASE,
    "sms.purchase",
    POST,
    "/purchase/sms",
    Multipart,
    Bearer,
    PaidMutation
);
endpoint!(
    SMS_CHECK,
    "sms.check",
    POST,
    "/sms/check",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    SMS_ACTIVE,
    "sms.active",
    POST,
    "/request/active",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    SMS_CANCEL,
    "sms.cancel",
    POST,
    "/sms/cancel",
    Multipart,
    BearerAndFormKey,
    Mutation
);
endpoint!(
    SMS_CANCEL_ALL,
    "sms.cancel_all",
    POST,
    "/sms/cancel_all",
    Multipart,
    Bearer,
    Mutation
);
endpoint!(
    SMS_CLEAR_CACHE,
    "sms.clear_cache",
    POST,
    "/sms/clear_cache",
    Multipart,
    Bearer,
    Mutation
);
endpoint!(
    SMS_ACTIVATE,
    "sms.activate",
    POST,
    "/sms/activate",
    Multipart,
    BearerAndFormKey,
    Mutation
);
endpoint!(
    SMS_REACTIVATE,
    "sms.reactivate",
    POST,
    "/sms/reactivate",
    Multipart,
    BearerAndFormKey,
    Mutation
);
endpoint!(
    SMS_ARCHIVE,
    "sms.archive",
    POST,
    "/request/archive",
    Multipart,
    BearerAndFormKey,
    Mutation
);
endpoint!(
    SMS_CHECK_RESEND,
    "sms.check_resend",
    POST,
    "/sms/check_resend",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    SMS_RESEND,
    "sms.resend",
    POST,
    "/sms/resend",
    Multipart,
    BearerAndFormKey,
    PaidMutation
);
endpoint!(
    SMS_STOCK,
    "sms.stock",
    POST,
    "/sms/stock",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    SMS_ALL_STOCK,
    "sms.all_stock",
    POST,
    "/sms/all_stock",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    SMS_HISTORY,
    "sms.history",
    POST,
    "/request/history",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    SMS_AREA_CODES,
    "sms.area_codes",
    POST,
    "/request/areacodes",
    FormUrlEncoded,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    PREORDER_RETRIEVE,
    "preorder.retrieve",
    POST,
    "/preorder/retrieve",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    PREORDER_CHECK,
    "preorder.check",
    POST,
    "/preorder/check",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    PREORDER_CANCEL,
    "preorder.cancel",
    POST,
    "/preorder/cancel",
    Multipart,
    BearerAndFormKey,
    Mutation
);
endpoint!(
    PREORDER_CREATE,
    "preorder.create",
    POST,
    "/preorder/create",
    Multipart,
    BearerAndFormKey,
    PaidMutation
);
endpoint!(
    PREORDER_PRICE,
    "preorder.price",
    POST,
    "/preorder/price",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    RENTAL_CATALOG,
    "rental.catalog",
    POST,
    "/rental/retrieve_all",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    RENTAL_PURCHASE,
    "rental.purchase",
    POST,
    "/purchase/rental",
    Multipart,
    BearerAndFormKey,
    PaidMutation
);
endpoint!(
    RENTAL_REFUND,
    "rental.refund",
    POST,
    "/rental/refund",
    Multipart,
    BearerAndFormKey,
    Mutation
);
endpoint!(
    RENTAL_EXTEND,
    "rental.extend",
    POST,
    "/rental/extend",
    Multipart,
    BearerAndFormKey,
    PaidMutation
);
endpoint!(
    RENTAL_AUTO_EXTEND,
    "rental.auto_extend",
    POST,
    "/rental/auto_extend",
    Multipart,
    BearerAndFormKey,
    Mutation
);
endpoint!(
    RENTAL_HISTORY,
    "rental.history",
    POST,
    "/rental/history",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    RENTAL_MESSAGES,
    "rental.messages",
    POST,
    "/rental/retrieve_messages",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    RENTAL_STATUS,
    "rental.status",
    POST,
    "/rental/retrieve_status",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    RENTAL_RESET,
    "rental.reset",
    POST,
    "/rental/reset",
    Multipart,
    BearerAndFormKey,
    Mutation
);
endpoint!(
    RENTAL_SERVICES,
    "rental.services",
    POST,
    "/rental/retrieve_services",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    RENTAL_ACTIVE,
    "rental.active",
    POST,
    "/rental/retrieve",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    RENTAL_PRICING,
    "rental.pricing",
    POST,
    "/rental/retrieve_pricing",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    RENTAL_STOCK,
    "rental.stock",
    POST,
    "/rental/stock",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    RENTAL_INFO,
    "rental.info",
    POST,
    "/rental/info",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    PRICING_ALL,
    "pricing.all",
    POST,
    "/request/pricing",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    PRICING_QUOTE,
    "pricing.quote",
    POST,
    "/request/price",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    CARRIER_LOOKUP,
    "carrier.lookup",
    POST,
    "/carrier/paid_lookup",
    Multipart,
    Bearer,
    PaidMutation
);
endpoint!(
    BUSINESS_UPDATE_USER,
    "business.update_user",
    POST,
    "/business/user/update",
    Multipart,
    Bearer,
    Mutation
);
endpoint!(
    BUSINESS_USER_HISTORY,
    "business.user_history",
    POST,
    "/business/user/history",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    BUSINESS_USERS,
    "business.users",
    GET,
    "/business/users",
    None,
    Bearer,
    ReadOnly
);
endpoint!(
    BUSINESS_CREATE_USER,
    "business.create_user",
    POST,
    "/business/create",
    Multipart,
    Bearer,
    Mutation
);
endpoint!(
    ESIM_PURCHASE,
    "esim.purchase",
    POST,
    "/esim/purchase",
    Multipart,
    BearerAndFormKey,
    PaidMutation
);
endpoint!(
    ESIM_PRICING,
    "esim.pricing",
    POST,
    "/esim/pricing",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    ESIM_HISTORY,
    "esim.history",
    POST,
    "/esim/history",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    ESIM_PLANS,
    "esim.plans",
    POST,
    "/esim/plans",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    ESIM_PROFILE,
    "esim.profile",
    POST,
    "/esim/profile",
    Multipart,
    BearerAndFormKey,
    ReadOnly
);
endpoint!(
    ESIM_DELETE,
    "esim.delete",
    POST,
    "/esim/delete",
    Multipart,
    BearerAndFormKey,
    Mutation
);
endpoint!(
    ESIM_TOP_UP,
    "esim.top_up",
    POST,
    "/esim/topup",
    Multipart,
    Bearer,
    PaidMutation
);
endpoint!(
    ESIM_TOP_UP_PLANS,
    "esim.top_up_plans",
    POST,
    "/esim/topup_plans",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    VOUCHER_GENERATE,
    "voucher.generate",
    POST,
    "/voucher/generate",
    Multipart,
    Bearer,
    PaidMutation
);
endpoint!(
    VOUCHER_RETRIEVE,
    "voucher.retrieve",
    POST,
    "/voucher/retrieve",
    Multipart,
    Bearer,
    ReadOnly
);
endpoint!(
    VOUCHER_DELETE,
    "voucher.delete",
    POST,
    "/voucher/delete",
    FormUrlEncoded,
    Bearer,
    Mutation
);
endpoint!(
    VOUCHER_BULK_GENERATE,
    "voucher.bulk_generate",
    POST,
    "/voucher/generate",
    Multipart,
    Bearer,
    PaidMutation
);

pub(crate) static ALL: [&Endpoint; 60] = [
    &CATALOG_SUCCESS_RATE,
    &CATALOG_BALANCE,
    &CATALOG_SUGGESTED_COUNTRIES,
    &CATALOG_SUGGESTED_POOLS,
    &CATALOG_COUNTRIES,
    &CATALOG_SERVICES,
    &CATALOG_POOLS,
    &SMS_PURCHASE,
    &SMS_CHECK,
    &SMS_ACTIVE,
    &SMS_CANCEL,
    &SMS_CANCEL_ALL,
    &SMS_CLEAR_CACHE,
    &SMS_ACTIVATE,
    &SMS_REACTIVATE,
    &SMS_ARCHIVE,
    &SMS_CHECK_RESEND,
    &SMS_RESEND,
    &SMS_STOCK,
    &SMS_ALL_STOCK,
    &SMS_HISTORY,
    &SMS_AREA_CODES,
    &PREORDER_RETRIEVE,
    &PREORDER_CHECK,
    &PREORDER_CANCEL,
    &PREORDER_CREATE,
    &PREORDER_PRICE,
    &RENTAL_CATALOG,
    &RENTAL_PURCHASE,
    &RENTAL_REFUND,
    &RENTAL_EXTEND,
    &RENTAL_AUTO_EXTEND,
    &RENTAL_HISTORY,
    &RENTAL_MESSAGES,
    &RENTAL_STATUS,
    &RENTAL_RESET,
    &RENTAL_SERVICES,
    &RENTAL_ACTIVE,
    &RENTAL_PRICING,
    &RENTAL_STOCK,
    &RENTAL_INFO,
    &PRICING_ALL,
    &PRICING_QUOTE,
    &CARRIER_LOOKUP,
    &BUSINESS_UPDATE_USER,
    &BUSINESS_USER_HISTORY,
    &BUSINESS_USERS,
    &BUSINESS_CREATE_USER,
    &ESIM_PURCHASE,
    &ESIM_PRICING,
    &ESIM_HISTORY,
    &ESIM_PLANS,
    &ESIM_PROFILE,
    &ESIM_DELETE,
    &ESIM_TOP_UP,
    &ESIM_TOP_UP_PLANS,
    &VOUCHER_GENERATE,
    &VOUCHER_RETRIEVE,
    &VOUCHER_DELETE,
    &VOUCHER_BULK_GENERATE,
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn registry_covers_the_generated_contract() {
        assert_eq!(ALL.len(), 60);
        assert_eq!(
            ALL.iter().filter(|item| item.method == Method::GET).count(),
            3
        );
        assert_eq!(
            ALL.iter()
                .filter(|item| item.method == Method::POST)
                .count(),
            57
        );
        assert_eq!(
            ALL.iter()
                .filter(|item| item.body_mode == BodyMode::Multipart)
                .count(),
            57
        );
        assert_eq!(
            ALL.iter()
                .filter(|item| item.body_mode == BodyMode::FormUrlEncoded)
                .count(),
            2
        );
        assert_eq!(
            ALL.iter()
                .filter(|item| item.body_mode == BodyMode::None)
                .count(),
            1
        );
    }

    #[test]
    fn every_descriptor_matches_the_generated_baseline() {
        let baseline: Value =
            serde_json::from_str(include_str!("../contracts/postman-baseline.json")).unwrap();
        let endpoints = baseline["endpoints"].as_array().unwrap();
        assert_eq!(endpoints.len(), ALL.len());

        for (offset, (descriptor, contract)) in ALL.iter().zip(endpoints).enumerate() {
            assert_eq!(contract["index"].as_u64(), Some((offset + 1) as u64));
            assert_eq!(
                Some(descriptor.method.as_str()),
                contract["method"].as_str()
            );
            assert_eq!(Some(descriptor.path), contract["path"].as_str());

            let expected_body = match contract["body_mode"].as_str().unwrap() {
                "formdata" => BodyMode::Multipart,
                "urlencoded" => BodyMode::FormUrlEncoded,
                "none" => BodyMode::None,
                mode => panic!("unexpected generated body mode: {mode}"),
            };
            assert_eq!(descriptor.body_mode, expected_body, "{}", descriptor.name);
            assert_eq!(contract["auth"].as_str(), Some("bearer"));

            let has_active_form_key =
                contract["body_fields"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|field| {
                        field["key"].as_str() == Some("key")
                            && field["disabled"].as_bool() == Some(false)
                    });
            let expected_auth = if has_active_form_key {
                AuthMode::BearerAndFormKey
            } else {
                AuthMode::Bearer
            };
            assert_eq!(descriptor.auth, expected_auth, "{}", descriptor.name);
        }
    }

    #[test]
    fn descriptor_names_are_unique_and_paths_are_relative() {
        let names: HashSet<_> = ALL.iter().map(|item| item.name).collect();
        assert_eq!(names.len(), ALL.len());
        assert!(ALL.iter().all(|item| item.path.starts_with('/')));
        assert!(ALL.iter().all(|item| !item.path.starts_with("//")));
    }

    #[test]
    fn authentication_and_safety_counts_match_the_reviewed_plan() {
        assert_eq!(
            ALL.iter()
                .filter(|item| item.auth == AuthMode::Bearer)
                .count(),
            26
        );
        assert_eq!(
            ALL.iter()
                .filter(|item| item.auth == AuthMode::BearerAndFormKey)
                .count(),
            34
        );
        assert_eq!(
            ALL.iter()
                .filter(|item| item.safety == SafetyClass::ReadOnly)
                .count(),
            36
        );
        assert_eq!(
            ALL.iter()
                .filter(|item| item.safety == SafetyClass::Mutation)
                .count(),
            14
        );
        assert_eq!(
            ALL.iter()
                .filter(|item| item.safety == SafetyClass::PaidMutation)
                .count(),
            10
        );
    }

    #[test]
    fn evidence_preserving_anomalies_remain_explicit() {
        assert_eq!(CATALOG_COUNTRIES.method, Method::GET);
        assert_eq!(CATALOG_COUNTRIES.body_mode, BodyMode::Multipart);
        assert_eq!(CATALOG_SERVICES.method, Method::GET);
        assert_eq!(CATALOG_SERVICES.body_mode, BodyMode::Multipart);
        assert_eq!(SMS_AREA_CODES.body_mode, BodyMode::FormUrlEncoded);
        assert_eq!(VOUCHER_DELETE.body_mode, BodyMode::FormUrlEncoded);
        assert_eq!(VOUCHER_GENERATE.path, VOUCHER_BULK_GENERATE.path);
        assert_ne!(VOUCHER_GENERATE.name, VOUCHER_BULK_GENERATE.name);
    }

    #[test]
    fn wire_request_debug_never_exposes_values() {
        let request = WireRequest {
            body_mode: Some(BodyMode::Multipart),
            body_fields: vec![("key".into(), "api-key-sentinel".into())],
            query_fields: vec![("phone".into(), "phone-sentinel".into())],
            raw_json: Some(serde_json::json!({"sms": "sms-sentinel"})),
        };
        let output = format!("{request:?}");
        assert!(output.contains("body_field_count: 1"));
        assert!(!output.contains("api-key-sentinel"));
        assert!(!output.contains("phone-sentinel"));
        assert!(!output.contains("sms-sentinel"));
    }
}
