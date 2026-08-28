# SMSPool Rust SDK

面向 Tokio/Axum 服务的异步 Rust SMSPool API SDK，声明 MSRV Rust 1.82。

## Current status

仓库已实现 contract-first SDK：60 个 Postman 操作均有静态 endpoint descriptor 和可调用方法，103/103 个响应 fixture 通过本地 mock 解码。Catalog、Pricing 和核心 SMS 闭环为稳定入口；证据较弱的操作显式放在 `Client::experimental()` 下，`/request/areacodes` 与四个 Voucher 操作保持原始 JSON 返回。

当前证据仍只来自仓库内 Postman collection：**未执行任何真实 SMSPool 请求、付费请求或 pilot**。因此仓库自动化通过只代表离线实现和契约一致性，不代表线上认证、限流、幂等性或生产恢复已验证。

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
    .build()?;
# Ok(())
# }
```

`Client` 可廉价 clone，且为 `Send + Sync + 'static`。API key、Authorization、号码、短信、密码、Voucher、eSIM 凭据和 typed response 中的任意 JSON fallback 不会进入默认 `Debug`/`Display`/tracing 输出；fallback 内容必须显式 `expose()`。

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

购买、充值、付费 lookup、续期和 Voucher 生成被标为 `PaidMutation`，SDK 永不自动重放。mutation 可能已送达但响应无法确认时返回 `Error::OutcomeUnknown`；应用必须通过 active/history 或业务账本对账，不能直接重试。

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

`ActiveOrdersWatcher` 对 `sms.active()` 顺序轮询，按 order 的 status/code/full-code 去重，并在插入前强制最大跟踪数量。订单从 active snapshot 消失即从内存指纹表移除。所有 polling 状态均为**进程内、非持久化**状态；生产服务必须外部持久化订单、截止时间、最终结果和重启恢复进度。

## Experimental and raw APIs

Preorder、Rental、Carrier、Business、eSIM、Voucher 及额外 SMS 操作位于 `client.experimental()`。这些类型反映 Postman 样例，不构成线上稳定性保证。通用 raw 请求仍经过相同的相对路径校验、认证、超时、响应大小限制、safety class 和重试边界，不能绕过传输策略。

当前仍保留 Postman 的 multipart GET 与混合 Bearer/form-key 认证证据，直到 `LIVE-001` 提供只读线上证明。

## Axum integration

核心 library 不依赖 Axum；Axum 仅为 dev-dependency，用于编译 [`examples/axum.rs`](examples/axum.rs)。该示例把 `Client` 放入 `State`，由应用自己映射错误，启动时不会请求 SMSPool；只有显式访问示例 route 才调用只读余额端点。

```bash
cargo check --example axum
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
```

`foundation` 与 `sdk` 是纯仓库/本地 mock 检查，不使用 API key。`LIVE-001`、`OPS-001`、`PILOT-001` 仍为 pending manual gates；`production` profile 在这些外部证据完成前应 fail closed。

## Documentation

- [`docs/api-contract.md`](docs/api-contract.md)：证据、实际 wire/decode 实现与未验证项
- [`docs/architecture.md`](docs/architecture.md)：实际模块、可靠性、polling 和 Axum 边界
- [`docs/production-acceptance.md`](docs/production-acceptance.md)：自动化与生产人工门槛
- [`acceptance/gates.json`](acceptance/gates.json)：机器可读验收 profile
- [`contracts/postman-baseline.json`](contracts/postman-baseline.json)：确定性生成的契约基线

不要手工修改 baseline、endpoint matrix 或 fixture；Postman 变化应通过 `scripts/postman_contract.py` 重新生成并审查 diff。
