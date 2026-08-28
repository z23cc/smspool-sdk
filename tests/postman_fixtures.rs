mod support;

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
    str::FromStr,
};

use serde::Deserialize;
use serde_json::Value;
use smspool::{
    api::{
        business::{BusinessUpdateRequest, CreateBusinessUserRequest},
        esim::EsimPageRequest,
        preorder::{CreatePreorderRequest, PreorderPriceRequest},
        pricing::PricingFilters,
        rental::RentalPurchaseRequest,
        sms::{
            AllStockFilters, AreaCodesRequest, HistoryRequest, PricingOption, PurchaseSmsRequest,
        },
        voucher::{BulkGenerateVouchersRequest, GenerateVoucherRequest},
    },
    ActivationToken, BusinessUserId, Client, CountryId, Days, Error, Money, OrderId, Password,
    PhoneNumber, PlanId, PoolId, PreorderId, RawFormValue, RentalCode, RentalId, RetryPolicy,
    ServiceId, TransactionId,
};
use support::{CapturedRequest, ResponseScript, Script, ScriptedServer};

const DESCRIPTOR_INDICES: [u16; 60] = [
    1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26,
    27, 28, 29, 30, 31, 32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50,
    51, 52, 53, 54, 55, 56, 57, 58, 59, 60,
];

#[derive(Debug, Deserialize)]
struct Manifest {
    fixtures: Vec<FixtureRow>,
}

#[derive(Debug, Deserialize)]
struct FixtureRow {
    file: String,
    status: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct ContractBaseline {
    endpoints: Vec<BaselineEndpoint>,
}

#[derive(Debug, Deserialize)]
struct BaselineEndpoint {
    index: u16,
    method: String,
    path: String,
    body_mode: String,
    auth: String,
    body_fields: Vec<BaselineField>,
    query_fields: Vec<BaselineField>,
}

#[derive(Debug, Deserialize)]
struct BaselineField {
    key: String,
    disabled: bool,
}

#[tokio::test]
async fn every_generated_postman_fixture_exercises_its_public_operation_offline() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/postman");
    assert!(root.join(".smspool-fixtures-generated").is_file());
    let manifest: Manifest = serde_json::from_slice(&fs::read(root.join("manifest.json")).unwrap())
        .expect("generated fixture manifest must be valid JSON");
    assert_eq!(manifest.fixtures.len(), 103);
    let baseline: ContractBaseline = serde_json::from_slice(
        &fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("contracts/postman-baseline.json"))
            .unwrap(),
    )
    .expect("generated contract baseline must be valid JSON");
    let baseline = baseline
        .endpoints
        .into_iter()
        .map(|endpoint| (endpoint.index, endpoint))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(baseline.len(), 60);
    assert_eq!(
        DESCRIPTOR_INDICES,
        std::array::from_fn(|index| index as u16 + 1)
    );

    let mut exercised = BTreeSet::new();
    let mut wire_exercised = BTreeSet::new();
    for row in &manifest.fixtures {
        assert!(
            exercised.insert(row.file.clone()),
            "duplicate fixture: {}",
            row.file
        );
        let path = root.join(&row.file);
        assert!(path.is_file(), "missing fixture: {}", path.display());
        let body = fs::read(&path).unwrap();
        let json: Value = serde_json::from_slice(&body).unwrap();
        let status = row.status.unwrap_or(200);
        let expected_api_error = status >= 300 || top_level_failure(&json);
        let index = fixture_index(&row.file);
        assert!(
            DESCRIPTOR_INDICES.contains(&index),
            "unknown endpoint fixture: {}",
            row.file
        );

        let server =
            ScriptedServer::start([Script::Respond(ResponseScript::bytes(status, body))]).await;
        let client = Client::builder("fixture-api-key")
            .base_url(server.base_url())
            .allow_insecure_http_for_mocking(true)
            .retry_policy(RetryPolicy::new(1).jitter_ratio(0.0))
            .build()
            .unwrap();
        let result = invoke(index, &client).await;
        if expected_api_error {
            assert!(
                matches!(result, Err(Error::Api(_))),
                "{} should be Error::Api, got {result:?}",
                row.file
            );
        } else {
            assert!(result.is_ok(), "{} should decode, got {result:?}", row.file);
        }
        server.wait_for_requests(1).await;
        assert_eq!(
            server.request_count(),
            1,
            "{} escaped single offline invocation",
            row.file
        );
        let captured = server.requests().pop().unwrap();
        assert_public_request(
            index,
            baseline.get(&index).expect("baseline endpoint must exist"),
            &captured,
        );
        wire_exercised.insert(index);
    }

    for index in DESCRIPTOR_INDICES {
        if wire_exercised.contains(&index) {
            continue;
        }
        let server = ScriptedServer::start([Script::Respond(ResponseScript::json(
            200,
            serde_json::json!({}),
        ))])
        .await;
        let client = Client::builder("fixture-api-key")
            .base_url(server.base_url())
            .allow_insecure_http_for_mocking(true)
            .retry_policy(RetryPolicy::new(1).jitter_ratio(0.0))
            .build()
            .unwrap();
        let result = invoke(index, &client).await;
        assert!(
            result.is_ok(),
            "request-only endpoint {index} should accept raw local JSON, got {result:?}"
        );
        server.wait_for_requests(1).await;
        let captured = server.requests().pop().unwrap();
        assert_public_request(
            index,
            baseline.get(&index).expect("baseline endpoint must exist"),
            &captured,
        );
        wire_exercised.insert(index);
    }

    assert_eq!(exercised.len(), manifest.fixtures.len());
    assert_eq!(wire_exercised, DESCRIPTOR_INDICES.into_iter().collect());
}

fn assert_public_request(index: u16, endpoint: &BaselineEndpoint, request: &CapturedRequest) {
    assert_eq!(request.method, endpoint.method, "endpoint {index} method");
    assert_eq!(request.target, endpoint.path, "endpoint {index} target");
    assert_eq!(endpoint.auth, "bearer", "endpoint {index} baseline auth");
    assert_eq!(
        request.header("authorization"),
        Some("Bearer fixture-api-key"),
        "endpoint {index} inherited Bearer auth"
    );

    let active_query = endpoint
        .query_fields
        .iter()
        .filter(|field| !field.disabled)
        .map(|field| field.key.as_str())
        .collect::<BTreeSet<_>>();
    assert!(
        active_query.is_empty(),
        "endpoint {index} baseline gained active query fields: {active_query:?}"
    );
    assert!(
        !request.target.contains('?'),
        "endpoint {index} sent query data"
    );

    let fields = match endpoint.body_mode.as_str() {
        "formdata" => multipart_fields(index, request),
        "urlencoded" => urlencoded_fields(index, request),
        "none" => {
            assert!(
                request.body.is_empty(),
                "endpoint {index} must have no body"
            );
            assert_eq!(request.header("content-type"), None);
            BTreeMap::new()
        }
        mode => panic!("endpoint {index} has unsupported baseline body mode {mode}"),
    };
    let active_body = endpoint
        .body_fields
        .iter()
        .filter(|field| !field.disabled)
        .map(|field| field.key.clone())
        .collect::<BTreeSet<_>>();
    assert_eq!(
        fields.keys().cloned().collect::<BTreeSet<_>>(),
        active_body,
        "endpoint {index} active public-operation fields differ from baseline"
    );
    for (name, actual) in fields {
        assert_eq!(
            actual,
            expected_field_value(index, &name),
            "endpoint {index} field {name}"
        );
    }
}

fn multipart_fields(index: u16, request: &CapturedRequest) -> BTreeMap<String, String> {
    let content_type = request
        .header("content-type")
        .unwrap_or_else(|| panic!("endpoint {index} multipart content type missing"));
    let boundary = content_type
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap_or_else(|| panic!("endpoint {index} wrong multipart content type: {content_type}"))
        .trim_matches('"');
    let marker = format!("--{boundary}");
    let text = request.body_text();
    let mut fields = BTreeMap::new();
    for raw_part in text.split(&marker).skip(1) {
        let part = raw_part.trim_start_matches("\r\n");
        if part.starts_with("--") || part.is_empty() {
            continue;
        }
        let (headers, value) = part
            .split_once("\r\n\r\n")
            .unwrap_or_else(|| panic!("endpoint {index} malformed multipart part"));
        let disposition = headers
            .lines()
            .find(|line| {
                line.to_ascii_lowercase()
                    .starts_with("content-disposition:")
            })
            .unwrap_or_else(|| panic!("endpoint {index} multipart disposition missing"));
        let name = disposition
            .split("name=\"")
            .nth(1)
            .and_then(|value| value.split('"').next())
            .unwrap_or_else(|| panic!("endpoint {index} multipart name missing"));
        let value = value.strip_suffix("\r\n").unwrap_or(value).to_owned();
        assert!(
            fields.insert(name.to_owned(), value).is_none(),
            "endpoint {index} duplicated multipart field {name}"
        );
    }
    fields
}

fn urlencoded_fields(index: u16, request: &CapturedRequest) -> BTreeMap<String, String> {
    assert_eq!(
        request.header("content-type"),
        Some("application/x-www-form-urlencoded"),
        "endpoint {index} urlencoded content type"
    );
    let mut fields = BTreeMap::new();
    for (name, value) in url::form_urlencoded::parse(&request.body) {
        assert!(
            fields
                .insert(name.into_owned(), value.into_owned())
                .is_none(),
            "endpoint {index} duplicated urlencoded field"
        );
    }
    fields
}

fn expected_field_value(index: u16, field: &str) -> &'static str {
    match field {
        "key" => "fixture-api-key",
        "service" | "country" | "service_id" | "plan" => "1",
        "rental" => "7",
        "pool" | "id" if matches!(index, 29 | 39 | 40) => "7",
        "pool" => "7",
        "id" => "1",
        "pricing_option" | "create_token" | "start" => "0",
        "quantity" if index == 60 => "2",
        "quantity" => "1",
        "areacode" | "exclude" => "[]",
        "activation_type" => "SMS",
        "carrier" => "fixture-carrier",
        "phonenumber" => "15551234567",
        "orderid" | "rental_code" => "ABCDEFGH",
        "length" => "20",
        "search" | "Search" => "",
        "max_price" | "balance" | "amount" => "1.00",
        "type" => "1",
        "days" => "30",
        "password" if index == 45 => "fixture-update-password",
        "password" => "fixture-password",
        "active" => "1",
        "username" => "fixture-user",
        "transactionId" => "ABCDEFGHI123456",
        "voucher" => "fixture-voucher",
        other => panic!("endpoint {index} has no deterministic expected value for {other}"),
    }
}

fn fixture_index(file: &str) -> u16 {
    file.get(..3)
        .and_then(|prefix| prefix.parse().ok())
        .unwrap_or_else(|| panic!("fixture path lacks numeric endpoint prefix: {file}"))
}

fn top_level_failure(value: &Value) -> bool {
    match value.as_object().and_then(|object| object.get("success")) {
        Some(Value::Bool(false)) => true,
        Some(Value::Number(number)) => number.as_i64() == Some(0),
        Some(Value::String(value)) => value == "0",
        _ => false,
    }
}

fn country() -> CountryId {
    CountryId::new("1").unwrap()
}

fn service() -> ServiceId {
    ServiceId::new("1").unwrap()
}

fn pool() -> PoolId {
    PoolId::new("7").unwrap()
}

fn order() -> OrderId {
    OrderId::new("ABCDEFGH").unwrap()
}

fn preorder() -> PreorderId {
    PreorderId::new("ABCDEFGH").unwrap()
}

fn rental_id() -> RentalId {
    RentalId::new("7").unwrap()
}

fn rental_code() -> RentalCode {
    RentalCode::new("ABCDEFGH").unwrap()
}

fn plan() -> PlanId {
    PlanId::new("1").unwrap()
}

fn transaction() -> TransactionId {
    TransactionId::new("ABCDEFGHI123456").unwrap()
}

fn money() -> Money {
    Money::from_str("1.00").unwrap()
}

async fn invoke(index: u16, client: &Client) -> Result<(), Error> {
    match index {
        1 => client.catalog().success_rates(&service()).await.map(drop),
        2 => client.catalog().balance().await.map(drop),
        3 => client
            .catalog()
            .suggested_countries(&service())
            .await
            .map(drop),
        4 => client
            .catalog()
            .suggested_pools(&service(), &country(), None)
            .await
            .map(drop),
        5 => client.catalog().countries().await.map(drop),
        6 => client.catalog().services(None).await.map(drop),
        7 => client.catalog().pools().await.map(drop),
        8 => {
            let request = PurchaseSmsRequest::new(country(), service())
                .pricing_option(PricingOption::Cheapest)
                .area_code(RawFormValue::new("[]").unwrap())
                .exclude(RawFormValue::new("[]").unwrap())
                .carrier("fixture-carrier")
                .unwrap()
                .phone_number(PhoneNumber::new("15551234567").unwrap());
            client.sms().purchase(&request).await.map(drop)
        }
        9 => client.sms().check(&order()).await.map(drop),
        10 => client.sms().active().await.map(drop),
        11 => client.sms().cancel(&order()).await.map(drop),
        12 => client.experimental().sms().cancel_all().await.map(drop),
        13 => client.experimental().sms().clear_cache().await.map(drop),
        14 => client
            .experimental()
            .sms()
            .activate(&order())
            .await
            .map(drop),
        15 => client
            .experimental()
            .sms()
            .reactivate(&order())
            .await
            .map(drop),
        16 => client.experimental().sms().archive().await.map(drop),
        17 => client
            .experimental()
            .sms()
            .check_resend(&order())
            .await
            .map(drop),
        18 => client.experimental().sms().resend(&order()).await.map(drop),
        19 => client
            .experimental()
            .sms()
            .stock(&country(), &service(), &pool())
            .await
            .map(drop),
        20 => client
            .experimental()
            .sms()
            .all_stock(&AllStockFilters::new())
            .await
            .map(drop),
        21 => client
            .sms()
            .history(&HistoryRequest::new(0, 20).unwrap())
            .await
            .map(drop),
        22 => client
            .experimental()
            .sms()
            .area_codes(&AreaCodesRequest::new(service(), country(), pool()))
            .await
            .map(drop),
        23 => client.experimental().preorder().retrieve().await.map(drop),
        24 => client
            .experimental()
            .preorder()
            .check(&preorder())
            .await
            .map(drop),
        25 => client
            .experimental()
            .preorder()
            .cancel(&preorder())
            .await
            .map(drop),
        26 => {
            let request = CreatePreorderRequest::new(
                service(),
                country(),
                pool(),
                RawFormValue::new("[]").unwrap(),
                money(),
            );
            client
                .experimental()
                .preorder()
                .create(&request)
                .await
                .map(drop)
        }
        27 => {
            let request = PreorderPriceRequest::new(service(), country(), pool());
            client
                .experimental()
                .preorder()
                .price(&request)
                .await
                .map(drop)
        }
        28 => client
            .experimental()
            .rental()
            .catalog(&RawFormValue::new("1").unwrap())
            .await
            .map(drop),
        29 => {
            let request =
                RentalPurchaseRequest::new(rental_id(), Days::new(30), service()).unwrap();
            client
                .experimental()
                .rental()
                .purchase(&request)
                .await
                .map(drop)
        }
        30 => client
            .experimental()
            .rental()
            .refund(&rental_code())
            .await
            .map(drop),
        31 => client
            .experimental()
            .rental()
            .extend(&rental_code(), Days::new(30))
            .await
            .map(drop),
        32 => client
            .experimental()
            .rental()
            .set_auto_extend(&rental_code())
            .await
            .map(drop),
        33 => client.experimental().rental().history().await.map(drop),
        34 => client
            .experimental()
            .rental()
            .messages(&rental_code())
            .await
            .map(drop),
        35 => client
            .experimental()
            .rental()
            .status(&rental_code())
            .await
            .map(drop),
        36 => client
            .experimental()
            .rental()
            .reset(&rental_code())
            .await
            .map(drop),
        37 => client
            .experimental()
            .rental()
            .services(&rental_id())
            .await
            .map(drop),
        38 => client.experimental().rental().active().await.map(drop),
        39 => client
            .experimental()
            .rental()
            .pricing(&rental_id())
            .await
            .map(drop),
        40 => client
            .experimental()
            .rental()
            .stock(&rental_id(), Days::new(30))
            .await
            .map(drop),
        41 => client
            .experimental()
            .rental()
            .info(&rental_code())
            .await
            .map(drop),
        42 => client.pricing().all(&PricingFilters::new()).await.map(drop),
        43 => client
            .pricing()
            .quote(&country(), &service(), &pool())
            .await
            .map(drop),
        44 => client
            .experimental()
            .carrier()
            .paid_lookup(&PhoneNumber::new("15551234567").unwrap())
            .await
            .map(drop),
        45 => {
            let request = BusinessUpdateRequest::new(BusinessUserId::new("1").unwrap())
                .password(Password::new("fixture-update-password").unwrap())
                .balance(money())
                .active(true);
            client
                .experimental()
                .business()
                .update_user(&request)
                .await
                .map(drop)
        }
        46 => client
            .experimental()
            .business()
            .user_history(&BusinessUserId::new("1").unwrap())
            .await
            .map(drop),
        47 => client.experimental().business().users().await.map(drop),
        48 => {
            let request = CreateBusinessUserRequest::new(
                "fixture-user",
                Password::new("fixture-password").unwrap(),
            )
            .unwrap();
            client
                .experimental()
                .business()
                .create_user(&request)
                .await
                .map(drop)
        }
        49 => client
            .experimental()
            .esim()
            .purchase(&plan())
            .await
            .map(drop),
        50 => client
            .experimental()
            .esim()
            .pricing(&EsimPageRequest::new(0, 20).unwrap())
            .await
            .map(drop),
        51 => client
            .experimental()
            .esim()
            .history(&EsimPageRequest::new(0, 20).unwrap())
            .await
            .map(drop),
        52 => client
            .experimental()
            .esim()
            .plans(&country())
            .await
            .map(drop),
        53 => client
            .experimental()
            .esim()
            .profile(&transaction())
            .await
            .map(drop),
        54 => client
            .experimental()
            .esim()
            .delete(&transaction())
            .await
            .map(drop),
        55 => client
            .experimental()
            .esim()
            .top_up(&transaction(), &plan())
            .await
            .map(drop),
        56 => client
            .experimental()
            .esim()
            .top_up_plans(&plan())
            .await
            .map(drop),
        57 => client
            .experimental()
            .voucher()
            .generate(&GenerateVoucherRequest::new(money()))
            .await
            .map(drop),
        58 => client.experimental().voucher().retrieve().await.map(drop),
        59 => client
            .experimental()
            .voucher()
            .delete(&ActivationToken::new("fixture-voucher").unwrap())
            .await
            .map(drop),
        60 => client
            .experimental()
            .voucher()
            .bulk_generate(&BulkGenerateVouchersRequest::new(money(), 2).unwrap())
            .await
            .map(drop),
        _ => panic!("missing descriptor registry entry: {index}"),
    }
}
