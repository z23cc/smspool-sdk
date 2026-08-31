# SMSPool Rust SDK

面向 Tokio/Axum 服务的异步 Rust SMSPool API SDK，库 MSRV 为 Rust 1.85。

## Current status

仓库已实现 contract-first SDK：60 个 Postman 操作均有静态 endpoint descriptor 和可调用方法，103/103 个响应 fixture 通过本地 mock 解码。Catalog、Pricing 和核心 SMS 闭环为稳定入口；证据较弱的操作显式放在 `Client::experimental()` 下，`/request/areacodes` 与四个 Voucher 操作保持原始 JSON 返回。`sms/all_stock` 对真实超大响应 fail-closed，返回 `UnsupportedOperation`；生产代码应使用有界的 `sms/stock`。

本仓库另有一份脱敏的人工 live 观察记录（[`acceptance/live-observations.json`](acceptance/live-observations.json)）：在明确授权且预算上限为 USD 0.50 的受控流程中，pricing/stock、一次购买、pending check、取消时间锁、后续取消和 USD 0.02 余额增量均被观察到；`all_stock` 超过 1 MiB 和 16 MiB 响应上限。该记录不含 key、号码、短信、完整订单 ID 或绝对余额，也**不是** revision-bound 的 `LIVE-001` 证据。因此自动化通过仍只代表离线实现和契约一致性，不能替代线上认证、幂等性、告警、恢复或 pilot 验收。

契约基线：60 个操作、103 个响应样例、5 个无响应样例操作，语义指纹 `c06cea363b24afd2dd8e3d564e543fe7fdc0610326d60b1102a9acbcadb3db7e`。完整清单见 [`docs/generated/endpoint-matrix.md`](docs/generated/endpoint-matrix.md)。

## Install and client setup

```toml
[dependencies]
smspool = { path = "." }
```

默认使用 Rustls 和 `https://api.smspool.net/`。SDK 自行构造禁用自动重定向的 reqwest client，避免 Bearer 或 form-key 经 307/308 泄露；不接受外部 client 注入。明文 HTTP 只允许通过显式 builder 选项访问 IPv4/IPv6 loopback IP mock，域名和非 loopback 地址即使显式启用也会被拒绝；loopback mock client 同时强制 `no_proxy()`，不会经过环境代理。

```rust,no_run
use smspool::Client;

# fn build() -> Result<(), smspool::Error> {
let client = Client::builder(std::env::var("SMSPOOL_API_KEY").unwrap())
    .max_concurrency(16)
    // Optional: pace every request attempt, including read-only retries.
    .max_requests_per_second(32)
    .qps_wait_timeout(std::time::Duration::from_secs(5))
    .build()?;
# Ok(())
# }
```

`Client` 可廉价 clone，且为 `Send + Sync + 'static`。API key、Authorization、号码、短信、密码、Voucher、eSIM 凭据和 typed response 中的任意 JSON fallback 不会进入默认 `Debug`/`Display`/tracing 输出；fallback 内容必须显式 `expose()`。QPS 限制是可选的、跨所有 `Client` clone 共享，并独立于 in-flight 并发上限；未配置时 SDK 不自行猜测供应商限额。SMSPool [官方文章](https://www.smspool.net/article/smspool-api-order-view-and-cancel-numbers-9883b6969fad) 提到 normal limit 为 32 req/s，实际部署仍应按账号/套餐验证并配置余量。生产应用还应在付费请求前写入自己的 intent ledger，响应 `OutcomeUnknown` 时只标记 `reconcile_only`，不要重放。

## Stable APIs

稳定入口为：

- `client.catalog()`：国家、服务、pool、余额、成功率与建议项；
- `client.pricing()`：价格清单和报价；
- `client.sms()`：单号码购买、短信查询、active 批量查询、取消、历史对账。

以下只读示例只有在实际执行时才会访问 SMSPool：

```rust,no_run
# async fn balance(client: &smspool::Client) -> Result<(), smspool::Error> {
let balance = client.catalog().balance().await?;
println!("balance={}", balance.balance);
# Ok(())
# }
```

购买、充值、付费 lookup、续期和 Voucher 生成被标为 `PaidMutation`，SDK 永不自动重放。mutation 可能已送达但响应无法确认时返回 `Error::OutcomeUnknown`；应用必须通过 active/history 或业务账本对账，不能直接重试。对取消这种有供应商时间锁的场景，可使用 `cancel_with_reconciliation`：只有调用方配置的**精确**错误签名才会等待并重试，所有其他错误和 `OutcomeUnknown` 都不会重放。

## Polling workflows

`wait_for_sms` 支持绝对 deadline、`CancellationToken`、指数间隔上限、jitter 和耗尽 transport retry 后的 `Retry-After` 降速。429 在收到 status/headers 后立即分类，不读取可能超大或停滞的响应体。`wait_for_code_with` 只调用业务方提供的提取器，SDK 不猜验证码。

```rust,no_run
use std::time::Duration;
use smspool::{wait_for_code_with, OrderId, PollOptions};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

# async fn poll(client: &smspool::Client) -> Result<(), Box<dyn std::error::Error>> {
let order_id = OrderId::new("provider-order-id")?;
let options = PollOptions::new(
    Instant::now() + Duration::from_secs(90),
    CancellationToken::new(),
)
.with_jitter_ratio(0.1)?;
let result = wait_for_code_with(client, &order_id, options, |text| {
    text.split_whitespace()
        .find(|part| part.len() == 6 && part.chars().all(|c| c.is_ascii_digit()))
        .map(str::to_owned)
})
.await?;
# let _ = result;
# Ok(())
# }
```

`ActiveOrdersWatcher` 对 `sms.active()` 顺序轮询，按 order 的 status/code/full-code 去重，并在插入前强制最大跟踪数量。订单从 active snapshot 消失即从内存指纹表移除。`cancel_with_reconciliation` 返回取消尝试次数、check/active/balance 观察和 `CancellationDisposition`，其中余额只作为带符号差额证据，不保存绝对余额。所有 polling 状态均为**进程内、非持久化**状态；生产服务必须外部持久化订单、截止时间、最终结果和重启恢复进度。

## Experimental and raw APIs

Preorder、Rental、Carrier、Business、eSIM、Voucher 及额外 SMS 操作位于 `client.experimental()`。这些类型反映 Postman 样例，不构成线上稳定性保证。通用 raw 请求仍经过相同的相对路径校验、认证、超时、响应大小限制、safety class 和重试边界，不能绕过传输策略。

当前仍保留 Postman 的 multipart GET 与混合 Bearer/form-key 认证行为；本次受控 live 观察只验证了选定核心请求，不能把全部 endpoint 的认证/编码组合升级为已验证事实。

## Axum integration

核心 library 默认不依赖 Axum；`postgres-example` feature 才启用 Axum、sqlx 和加密依赖，用于编译 [`examples/axum.rs`](examples/axum.rs)。`Client` 可直接放入 Axum `State`。示例启动时不会自动购买号码；它把 provider order ID 加密后写入 PostgreSQL，只持久化轮询元数据，并在重启后通过租约/版本重新 claim 非终态订单。付费请求前应记录 intent，成功后使用示例的 `record_purchase_for_intent` 原子绑定订单并 resolve；`OutcomeUnknown` 只能进入 `reconcile_only`。

```bash
# 仅编译示例（不连接数据库）
cargo check --example axum --features postgres-example
# 运行示例需要 DATABASE_URL、SMSPOOL_API_KEY、SMSPOOL_ORDER_KEY（64 hex chars）
cargo run --example axum --features postgres-example
# 真实 PostgreSQL 恢复演练（显式 ignored，不属于普通离线 suite）
DATABASE_URL=postgres://... SMSPOOL_ORDER_KEY=$(openssl rand -hex 32) \\
  cargo test --features postgres-example --test postgres_recovery -- --ignored --nocapture
```

## Validation

```bash
python3 scripts/postman_contract.py check
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test --test postman_fixtures # 103 responses + all 60 request wires
cargo test --test transport_contract
cargo test --test redaction
cargo test --test workflow
./scripts/acceptance.sh sdk
python3 scripts/acceptance.py validate  # also validates sanitized live observations
```

`foundation` 与 `sdk` 是纯仓库/本地 mock 检查，不使用 API key。`acceptance/live-observations.json` 只记录可审计的脱敏事实，不能被 `acceptance/evidence.json` 验证器当作 manual attestation。`LIVE-001`、`OPS-001`、`PILOT-001` 仍为 pending manual gates；`production` profile 在这些外部证据完成前应 fail closed。

## Documentation

- [`docs/api-contract.md`](docs/api-contract.md)：证据、实际 wire/decode 实现与未验证项
- [`docs/architecture.md`](docs/architecture.md)：实际模块、可靠性、polling 和 Axum 边界
- [`docs/production-acceptance.md`](docs/production-acceptance.md)：自动化与生产人工门槛
- [`docs/production-research.md`](docs/production-research.md)：外部资料矛盾、剩余风险与下一阶段验收
- [`acceptance/gates.json`](acceptance/gates.json)：机器可读验收 profile
- [`contracts/postman-baseline.json`](contracts/postman-baseline.json)：确定性生成的契约基线

不要手工修改 baseline、endpoint matrix 或 fixture；Postman 变化应通过 `scripts/postman_contract.py` 重新生成并审查 diff。

项目文档：

- [`CHANGELOG.md`](CHANGELOG.md)：变更记录与**已知限制**
- [`CONTRIBUTING.md`](CONTRIBUTING.md)：开发流程、契约再生成、发布检查清单
- [`SECURITY.md`](SECURITY.md)：默认保护范围、**不**保护的部分与运维规则

## Feature flags

| Flag | 默认 | 说明 |
|---|---|---|
| `rustls-tls` | ✅ | 通过 rustls 提供 TLS |
| `tracing` | | 低基数 `tracing` 事件；不记录请求体或凭据 |
| `postgres-example` | | 仅供 Axum/PostgreSQL 示例与其恢复测试使用，非库功能 |

## MSRV

库 MSRV 为 Rust **1.85.0**，由 CI 以 `cargo check --locked --lib` 校验。

`postgres-example` 是仅供示例使用的 dev feature，其 sqlx 0.9 依赖声明 `rust-version = 1.94`，因此启用它需要更新的工具链。这不影响库本身的 MSRV —— 该 feature 不属于发布产物的必需路径。

MSRV 变更按 minor 版本处理。

## License

[Apache License 2.0](LICENSE-APACHE)，版权声明见 [`NOTICE`](NOTICE)。

除非另有明确声明，你有意提交给本项目的任何贡献，均按 Apache-2.0 授权，无附加条款。
