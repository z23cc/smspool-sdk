# SDK architecture

## Goals

- 为 Tokio/Axum 服务提供 `Clone + Send + Sync + 'static` 的异步 SMSPool client。
- 忠实表达不规则 wire contract，同时给业务代码稳定、类型安全的 Rust API。
- 对付费 mutation 保守处理，不因自动重试制造重复订单或重复扣费。
- 在供应商新增字段或状态时继续工作，并为未知内容保留诊断证据。
- 把 HTTP、凭据和响应中的客户数据从默认日志中隔离。

非目标：SDK 不负责业务数据库、任务队列、跨进程恰好一次、用户授权、HTTP API 状态映射或验证码业务规则。

## Layer model

```text
Axum application / durable jobs
              │
      high-level workflows       wait_for_sms, reconciliation helpers
              │
         resource APIs           catalog, sms, preorder, rental, ...
              │
       endpoint descriptors      method, path, body, auth, safety class
              │
        transport pipeline       timeout, bounded body, decode, retry
              │
          reqwest client         TLS, pooling, DNS, proxy
```

当前为单 crate，核心模块如下：

```text
src/
├── lib.rs
├── client.rs
├── endpoint.rs
├── transport.rs
├── error.rs
├── de.rs
├── types.rs
├── poll.rs
└── api/
    ├── catalog.rs
    ├── sms.rs
    ├── preorder.rs
    ├── rental.rs
    ├── pricing.rs
    ├── carrier.rs
    ├── business.rs
    ├── esim.rs
    └── voucher.rs
```

## Client ownership

`Client` 包含 `Arc<Inner>`，内部持有复用的 `reqwest::Client`、secret API key、base URL、超时、并发限制和 retry policy。它必须：

- 可廉价 clone；
- 默认使用 HTTPS base URL；
- 允许显式 loopback-IP mock base URL，但拒绝外部 reqwest client 注入；
- 手写或派生安全的 `Debug`，永不显示 API key；
- 对响应体设置大小上限；
- 不把 request body 加入 tracing span。

base URL 覆盖是显式 builder 选项。SDK 始终自行构造 `redirect(Policy::none())` 的 reqwest client；HTTP mock 模式仅接受 IPv4/IPv6 loopback IP、不接受域名或非 loopback 主机，并强制 `no_proxy()`，从构造边界阻止凭据被自动重定向、经环境代理转发或明文发送到外部地址。

## Endpoint descriptors

全部 60 个操作使用 `src/endpoint.rs` 中的静态 descriptor，包含：

```rust
struct Endpoint {
    name: &'static str,
    method: Method,
    path: &'static str,
    body_mode: BodyMode,
    auth: AuthMode,
    safety: SafetyClass,
}

enum SafetyClass {
    ReadOnly,
    Mutation,
    PaidMutation,
}
```

重试、编码和认证由 descriptor 驱动，不能散落在 60 个方法中靠人工约定。共享 route 的两个 Voucher 操作可以使用不同 descriptor 和请求类型，但 wire path 相同。

## Transport pipeline

统一执行顺序：

1. 本地校验请求并按 descriptor 编码。
2. 注入认证；对日志中的 header、form 和 URL 做脱敏。
3. 获取全局并发 permit。
4. 发送带连接/请求 timeout 的请求。
5. 收到 429 status/headers 后立即返回或重试，不读取其 body；其他响应才有界读取 body。
6. 结合 HTTP status 与顶层 `success` 判定错误。
7. 使用路径感知的 serde 解码目标类型；mutation 的成功响应不可读或不可解码且请求可能已送达时返回 `OutcomeUnknown`。
8. 仅在 `ReadOnly` 且错误满足 policy 时执行有上限、带 jitter 的重试。

429 应优先尊重合法 `Retry-After`；不存在该 header 时使用可配置退避。分类发生在 `Content-Length` 和 body stream 之前，因此超大/停滞 429 body 不会覆盖 `RateLimited` 语义。mutation 收到 429 直接返回，不自动重发。

## Type system

- `Money(Decimal)` 表达 API 金额单位。
- `Cents(u64)` 只表达明确标为 cents 的字段。
- `OrderId`、`RentalCode`、`PlanId`、`CountryId`、`ServiceId`、`PoolId` 等 newtype 防止参数串位，同时宽容读取字符串/整数 ID。
- `UnixTimestamp`、`Seconds`、`Hours`、`VendorDateTime` 明确保留不同 wire 单位/格式。
- `StatusValue` 保留数字、字符串、布尔、null 和其他未知 JSON；`Other(Value)` 及 typed response 的其他任意 JSON fallback 使用脱敏 `Debug`，只能显式暴露。结构化状态只按已观察字段分类，不猜 vendor 数值语义。
- `DecodedJson<T>` 同时支持解析双重编码 JSON 和保留不可解析原文。

公开响应 struct 和错误枚举使用 `#[non_exhaustive]`；新增服务端字段不构成解码失败。

## Error model

顶层错误类型至少区分：

- `Api(ApiError)`
- `RateLimited`
- `Transport`
- `Timeout`
- `Decode`
- `InvalidRequest`
- `OutcomeUnknown`

`ApiError` 保存 HTTP status、机器类型码、参数错误、可选业务附加数据和有界 raw value。message 可能包含 HTML，只作为不可信文本展示；不得直接拼进应用 HTML。

`OutcomeUnknown` 只用于非幂等请求可能已经被服务端接收的场景，包括发送后断连、timeout，以及 HTTP 成功但 mutation 结果损坏或无法解码。它是需要对账的状态，不等价于“操作失败”。发送前即可证明失败的本地/连接错误仍可返回普通错误。

## Reliability model

### Retry boundary

| 场景 | 自动重试 |
|---|---|
| ReadOnly + 429/选定 5xx/发送前连接失败 | 有上限地重试 |
| ReadOnly + decode error | 默认不重试 |
| Mutation/PaidMutation + 任意 transport 异常 | 不重试 |
| 明确 API 业务错误 | 不重试 |

对于 reqwest 无法可靠判断请求是否已到达服务端的 mutation transport 异常，保守返回 `OutcomeUnknown`。应用随后使用 active/history 等查询对账；没有供应商幂等键时，SDK 不承诺 exactly-once。

### Polling

`src/poll.rs` 已实现 `wait_for_sms`：使用绝对 Tokio deadline、`CancellationToken`、递增到最大值的 interval、可关闭的有界 jitter，并在 transport 重试耗尽后把合法 `Retry-After` 作为 delay floor。请求与 sleep 都可被取消；deadline/cancel error 保留最后一次 `SmsCheck`。它返回完整 Received/Terminated 状态，不猜验证码。`wait_for_code_with(extractor)` 优先把明确暴露的 `full_sms`（否则 `sms`）交给调用方，返回 terminal snapshot 与可选提取结果。

高并发场景可使用 `ActiveOrdersWatcher`：

- 顺序调用稳定的 `sms.active()`，共享 client transport 并发上限；
- 使用相同 deadline、cancellation、jitter 和 429 delay 规则；
- 仅在 status/code/full-code 变化时发出事件，抑制重复 snapshot；
- 对唯一 active order 数设置构造时上限，超出时在修改 map 前返回本地错误；
- snapshot 中消失的 order 会从 fingerprint map 删除，再次出现时重新发出。

观察器不推断 vendor terminal status，也不产生“完成”事件；从 active 列表消失只代表停止本地跟踪。其 fingerprint、pending event 和 deadline 全为进程内状态，重启即丢失。

## Axum boundary

核心 library dependency 不包含 Axum；`Cargo.toml` 仅将 Axum 放在 dev-dependencies 以编译 `examples/axum.rs`。`Client: Clone` 已足以放入 `State`：

```rust
#[derive(Clone)]
struct AppState {
    smspool: smspool::Client,
    // db, queue, metrics ...
}
```

应用负责把自身 `AppState` 映射到 client，并根据产品语义把 SDK error 转成 HTTP response。SDK 不默认实现 `IntoResponse`，因为余额不足、供应商认证失败和缺货在不同产品中可能对应不同状态码。

生产应用必须将订单 ID、过期时间、轮询进度和最终结果持久化。仅用 `tokio::spawn` 的轮询会在进程重启后丢失，不能作为生产任务系统。

## Security and observability

默认禁止记录 API key、Authorization、请求表单、号码、短信、验证码、eSIM token、ICCID 和完整 raw body。可观测数据采用 endpoint 名称和低基数错误类别：

- 请求时延、状态类别和供应商 429；
- retry 次数及原因；
- timeout、decode error、OutcomeUnknown；
- 并发 permit 等待时长；
- 轮询完成/过期耗时。

订单标识如需关联日志，使用不可逆 hash 或仅显示固定长度后缀。tracing 和 metrics feature 均不能改变协议行为。

## Compatibility and release policy

- 初始稳定范围只包含 Catalog 和核心 SMS 闭环。
- 无响应样例或未在线验证的模块标记 experimental。
- `raw()` 允许在 SDK 更新前访问新增端点，但仍经过认证、超时、大小限制和脱敏策略。
- Postman 更新先生成并审查 contract diff，再修改类型与测试。
- public API 的破坏性修改遵循 semver；wire contract 的兼容性与 crate semver 分开记录。

## Implementation status

- `client`、`endpoint`、`transport`、`error`、`de`、`types` 和九个 resource module 已实现。
- 60/60 operations 有 descriptor 和公共稳定/experimental 调用路径；离线 acceptance 对 103/103 response fixture 解码，并对全部 60 个公共操作逐一校验 baseline method/path/body mode/inherited Bearer/active field names 与确定性 field values。
- transport/auth/retry/OutcomeUnknown、redaction、polling/workflow 和 Axum example 均有本地编译或测试覆盖。
- `examples/axum.rs` 只示范 State 与应用自有错误映射；启动本身不请求 SMSPool，且不冒充持久化生产服务。
- 尚未执行受控只读 live smoke；线上认证组合、multipart GET、真实限流、运维恢复和 pilot 仍属于 LIVE/OPS/PILOT 人工证据。
