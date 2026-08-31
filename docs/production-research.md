# SMSPool SDK 生产研究与后续验收

> 研究日期：2026-08-29（Asia/Shanghai）
>
> 本文是风险研究和验收计划，不是生产认证。仓库当前仍以 `LIVE-001`、`OPS-001`、`PILOT-001` 为 pending manual gates；任何外部资料与 Postman collection 冲突时，必须保存 revision-bound 的线上证据后才能改变 SDK wire 行为。

## 1. 证据分层

| 层级 | 当前来源 | 可以证明 | 不能证明 |
|---|---|---|---|
| A | `postman.json`、`contracts/postman-baseline.json`、103 个 fixture | 60 个操作的静态字段、样例响应和现有请求 wire | 线上认证组合、限流、幂等、容量 |
| B | 本仓库 mock、Rust 测试、真实 PostgreSQL ignored test | transport 边界、`OutcomeUnknown`、QPS、取消状态机、租约/重启数据语义 | SMSPool 线上可用性和生产告警 |
| C | `acceptance/live-observations.json` | 一次受控操作员观察的脱敏事实 | 可复现性、revision 绑定、全 endpoint 覆盖 |
| D | 正式 live/OPS/pilot evidence | 指定版本、账号和环境下的线上验收 | 对未来供应商行为的永久保证 |

## 2. 外部资料与 collection 的矛盾

### 2.1 请求方法

SMSPool 2026 年官方 API 文章称 API 请求使用 HTTPS POST + form data，并以 `purchase/sms`、`sms/check`、`request/active` 为主要流程；旧版官方文章则写明除另有说明外 GET/POST 均可。当前 Postman collection 仍把 `country/retrieve_all`、`service/retrieve_all` 和 `business/users` 定义为 GET，其中前两个还带 multipart body。

因此 SDK 当前忠实保留 Postman descriptor，不在没有线上证据时偷偷切换为 POST 或 query。`LIVE-001` 必须至少验证：

- GET+multipart 是否被服务端接受；
- POST+form 与 query 变体是否返回同一语义；
- 认证头、form `key` 和无 key 公开操作的边界。

来源： [SMSPool 2026 API flow article](https://www.smspool.net/article/smspool-api-order-view-and-cancel-numbers-9883b6969fad)、[SMSPool API usage article](https://gaa6sluzg5hu8xh.smspool.net/article/how-to-use-the-smspool-api-0dd6eadf4c?PageSpeed=noscript)。

### 2.2 状态码

较新的官方流程文章只列出 `1=pending`、`3=complete`、`6=refunded`；旧版资料列出 `1..8`，还包括 expired、cancelled、resend、processing、activating。当前 SDK 保留 `StatusValue` 原值，并按响应字段形态构造 `SmsCheck`，但尚未把全部数字状态升级为稳定枚举。

在正式 live 证据覆盖 `2/4/5/7/8` 之前，应用不得把未知 status 当作成功、退款或可再次购买。后续建议增加显式 `SmsStatus`/`is_terminal()`，并用 synthetic fixtures 验证无 `message` 的终态不会无限 pending。

### 2.3 Webhook

Postman/API collection 没有 webhook 管理或签名协议，但 SMSPool 另有官方设置页文章描述 custom JSON webhook，并说明连续 50 次失败会被停用、月租号码暂不支持。另一篇 API flow 文章仍以“没有 push，需要 polling”描述核心 API。

结论：webhook 是账户设置层的可选外部通知，不是当前 SDK 已验证的 API contract。生产 Axum 不得把未验证 webhook 当作唯一交付路径；若接入，必须由 consuming application 自行提供：

- 独立 ingress secret 或 mTLS/IP 策略（供应商文章未给出签名校验）；
- body size limit、JSON schema version、重放/重复事件去重；
- `orderid`/`rental_code` 的不可逆关联，不记录短信正文到普通日志；
- webhook 失败后的 polling fallback 和告警。

来源： [SMSPool webhook setup article](https://gaa6sluzg5hu8xh.smspool.net/article/how-to-setup-webhooks-for-smspool-ec19b80ade92)、[SMSPool API flow article](https://www.smspool.net/article/smspool-api-order-view-and-cancel-numbers-9883b6969fad)。

### 2.4 限流

官方资料说明普通账号通常为 32 requests/second，并建议用 `request/active` 替代大量 `sms/check`；旧版资料还说明失败请求超过 300 requests/second 时，即使 Business 账号也可能被限流一分钟。SDK 的 `max_requests_per_second` 是本地 start-rate limiter，不能证明账号配额，也没有单独的 provider failed-request budget。

生产配置应按账号类型设置保守余量，并把 429、失败请求突增、`Retry-After` 和 QPS admission timeout 作为独立指标。不可将 Business 账号理解为“无限速”。

来源： [SMSPool API usage article](https://gaa6sluzg5hu8xh.smspool.net/article/how-to-use-the-smspool-api-0dd6eadf4c?PageSpeed=noscript)、[SMSPool 2026 API flow article](https://www.smspool.net/article/smspool-api-order-view-and-cancel-numbers-9883b6969fad)。

## 3. 当前实现的剩余高风险

### R1 — intent 与订单落库不是同一事务（高，已部分修复）

此前 `examples/postgres_order_store.rs` 允许调用方先 `record_purchase_intent`，再单独 `record_purchase` 和 `resolve_purchase_intent`。本轮已增加 `record_purchase_for_intent`：在同一个数据库事务中插入/绑定订单并更新 intent，重复调用保持幂等，单一 intent 不能绑定第二个 provider order；旧 API 仍保留兼容。

**剩余验收要求：** 增加进程崩溃注入和数据库断连测试，证明任意提交边界都不会出现 resolved-without-order；`OutcomeUnknown` 仍只能进入 `reconcile_only`，不能自动创建第二个 paid intent。

### R2 — 租约没有和 provider 请求超时形成强约束（高）— 已修复构造期约束

初次记录时估计为「lease 30s vs request_timeout 30s」。复核后实际差距远大于此：单次 read-only 调用的最坏在途时间是

```
max_attempts x (qps_wait_timeout + concurrency_wait_timeout + request_timeout)
  + (max_attempts - 1) x max(max_delay, max_retry_after)
= 3 x (5s + 5s + 30s) + 2 x 30s = 180s   # examples/axum.rs 的配置
```

该 180s 是**示例自身配置**下的数值（`qps_wait_timeout(5)` + 其余默认值），不是通用常量；换一组超时就会得到另一个上界，这正是要从运行时配置推导而非写死的原因。示例原先硬编码的 30 秒 lease 因此小了约 6 倍，且 `examples/axum.rs` 中没有任何 renew/heartbeat。

**本轮已完成：** 新增 `Client::max_in_flight_duration()` 暴露该上界；`OrderStore::from_pool` 增加必填 `min_lease` 参数，`lease <= min_lease` 直接返回 `StoreError::LeaseTooShort`（相等也拒绝，因为过期与完成会竞争）；`examples/axum.rs` 改为由 client 推导 lease 并保留 2 倍余量。由于上界从实时配置计算，调整任一 timeout 都不会静默削弱该约束。回归测试见 `tests/postgres_recovery.rs` 的 `lease_shorter_than_worst_case_in_flight_is_rejected` 与 `worst_case_in_flight_accounts_for_retries_and_backoff`。

**剩余验收要求：** 仍需注入超过 lease 的 scripted response，确认只允许一个终态写入、旧 worker 得到 `StaleClaim`；构造期约束不能替代请求期续租。

### R3 — 终态 status 的形态推断仍可能漂移（中高）— 亏钱路径已封堵

`SmsCheck` 当前优先依据 `sms/full_sms/message` 字段构造变体，完全不看 `status`。两个方向的后果并不对称：

- 漏判终态（返回 numeric terminal status 但省略 `message`）→ 继续 pending 到 deadline，只浪费轮询；
- **误判终态**（pending 订单带一句提示性 `message`）→ 在 `cancel_with_reconciliation` 中被判为 `Terminated` → `TerminalSms` → **停止继续取消，号码继续挂着且不退款**。

线上已确认该供应商会在响应里返回人话（`sms/cancel` 的时间锁提示即是一例），因此第二种并非假想。

**本轮已完成：** 时间锁分支中，`Terminated` 只有在 `request/active` **明确报告 Absent** 时才结束取消。`Present` 表示两个观察互相矛盾，`Unavailable`（含解码失败）表示无法确认 —— 两者都继续有界重试。`disposition_from_observations` 同步改为三分支，`Unavailable` 归为 `Inconclusive` 而非 `TerminalSms`。`Received` 由真实短信内容支撑，无需佐证。回归测试见 `tests/workflow.rs` 三个 `cancellation_*_active_*` 用例。

**后续部分完成（本轮，属于「固化当前行为」而非「验证供应商语义」）：** 新增 `SmsCheck::status()` / `status_code()` / `is_terminal()`，原始 status 对三个变体都完整保留；并为 status 1/2/3/4/5/6/7/8 × 各种字段组合补齐 synthetic 断言（`numeric_status_classification_is_pinned_for_every_known_and_unknown_code`），钉死每种形态的归类结果，非数字 status 也覆盖。

关键取舍：**无证据的 status（2/4/5/7/8）若不带 `message` 也不带短信内容，一律归为 pending**，宁可轮询到 deadline，也不靠猜把订单判成终态。`is_terminal()` 因此是形态推断而非 status 白名单 —— 这一点已写进 rustdoc，避免调用方误以为它有供应商语义。

**必须明确：** 这些 synthetic 断言固化的是 **SDK 的保守猜测**，不是供应商真相。若 status 4 实际是「无 message 的终态」，现有测试反而把「继续轮询到 deadline」钉成了预期行为。这是一个**有意的默认选择**，不是一次验证。

**剩余验收要求：** 真实终态 status 的语义仍需线上证据；`docs`/rustdoc 不得暗示 2/4/5/7/8 已有已知含义。

### R10 — 收码（happy path）（高）— 解码路径已用真实数据验证，端到端时序仍待真实签注

`SmsCheck::Received`、`SmsText` 解码、`full_sms` 与 `sms` 的差异、`wait_for_sms` 终态判定，至今**只有 fixture 覆盖**：历次线上测试都在 pending 状态取消，从未真正收到过短信。而收码正是该产品的核心用途。

该缺口无法由本仓库单方面关闭：需要一次**操作者本人合法使用**的真实注册流程来触发短信。用虚拟号去随机第三方服务过验证通常违反对方条款，不作为验收手段；「自行向该号码发短信」则因国际短信常被拦截，负面结果没有判别力。

**本轮已完成：** 新增 `tests/live_receive.rs`（`#[ignore]` + 显式 opt-in 环境变量）。它在价格上限内下单、打印号码、用 `wait_for_sms` 等待，并输出**结构性事实**（各字段是否存在、长度、`sms` 与 `full_sms` 是否不同），短信正文仅在 `SMSPOOL_LIVE_PRINT_CODE=1` 时打印；未收到则自动走 `cancel_with_reconciliation` 退款。等待窗口受供应商 `expires_in` 约束。

**本轮已验证（只读，未额外付费）：** 账号历史中已存在两笔 `completed` 订单，直接对其调用 `sms/check`，用真实供应商数据确认了收码解码路径：

- 归类为 `SmsCheck::Received`，`is_terminal()` 为 true，`status_code()` 为 `3`；
- `sms` 与 `full_sms` **同时存在且内容不同**，`full_sms` **包含** `sms`（即 `sms` 是抽取出的验证码，`full_sms` 是整条报文）；
- 真实报文是**多字节 UTF-8**（样本分别为 35 字符/71 字节、22 字符/38 字节），解码无损 —— 而仓库内全部 Postman fixture 均为 ASCII，此前从未覆盖该路径；
- 对真实客户内容渲染 `Debug`，**未泄漏**验证码或报文正文；
- 同轮被退款的订单正确归类为 `Terminated`。

已补 `received_sms_decodes_multibyte_content_without_leaking_it` 固化该形态与脱敏保证。

**仍未验证：** 本轮为收码专门购买的号码（印尼/Swiggy）在 240 秒窗口内**未收到任何入站短信**（操作者自行发送的国际短信未送达，属链路问题，不能据此判断供应商服务流量）。因此「下单 → 真实业务短信到达 → `wait_for_sms` 返回 Received」这一**端到端时序**仍未跑通，只验证了其中的解码与判定环节。

**剩余验收要求：** 在一次真实签注中跑通端到端时序；`LIVE-001` 在此之前保持 pending。

### R8 — `request/pricing` 无过滤时超出响应上限（高）— 已修复

线上实测（curl，单次采样）：

| 端点 | 字节数 | 相对 1 MiB 默认上限 |
|---|---:|---|
| `pool/retrieve_all` | 153 | 安全 |
| `country/retrieve_all` | 11,440 | 安全 |
| `service/retrieve_all` | 60,599 | 安全 |
| `request/pricing`（`max_price=0.02`） | 1,453 | 安全 |
| `request/pricing`（`country=1`） | 334,959 | 32% |
| `request/pricing`（`max_price=0.10`） | 570,632 | 54% |
| **`request/pricing`（无过滤）** | **18,080,463** | **约 17.2 倍，必然失败** |

`pricing().all()` 属于**已声明稳定**的 API，但无过滤调用对任何使用默认配置的调用方都只会返回 `ResponseTooLarge`。这与 `all_stock` 是同一类缺陷，只是发生在稳定面而非 experimental 面。

**本轮已完成：** `PricingApi::all` 在本地拒绝空过滤条件，返回 `Error::InvalidRequest`，在发出请求前给出可操作错误。回归测试 `unfiltered_pricing_is_rejected_before_any_request_is_sent` 同时断言「未发出任何请求」。

**注意：** 过滤是必要条件而非充分条件 —— `max_price=0.10` 已占默认上限的 54%，稍微放宽就会溢出。文档已标注实测值并指向 `max_response_bytes`。

**注意过滤本身不是保证：** `max_price=100` 这类宽松过滤同样能拿回完整目录，本地守卫只拦截「完全没有过滤」这一必然失败的情况。

**剩余验收要求：** 目录规模会随供应商变化，应在生产侧对 `ResponseTooLarge` 建立告警，而不是依赖这次采样。以上字节数为只读探测的单次采样，未写入 `acceptance/live-observations.json` —— 该文件的记录结构面向付费订单闭环，强行塞入尺寸探测会导致 `purchase_decoded` 等字段失真。

### R9 — 取消流程会被单次 429 中止（高）— 已修复

`sms/cancel` 是 Mutation，transport 层不重试；而 `cancel_with_reconciliation` 的通用错误分支会把 `Error::RateLimited` 当作终止条件，在**取消预算和 deadline 都还有剩余**的情况下直接返回，留下一个仍然存活且未退款的号码。429 的语义是「未执行，稍后重试」，与时间锁同类，不应终止流程。

考虑到本地 QPS 限流是每进程的（10 实例 × 32 rps 打到同一账号），这条比已修复的漂移场景更容易触发。

**本轮已完成：** 新增独立的 `Err(Error::RateLimited { retry_after, .. })` 分支，遵循 `Retry-After` 等待后继续，受 `max_cancel_attempts` 和 deadline 约束；同时把终态判定抽出为 `settled_disposition` / `exhausted_disposition`，让时间锁与限流两条路径不会各自漂移。回归测试见 `cancellation_survives_a_rate_limited_attempt` 与 `persistent_rate_limiting_stops_at_the_cancel_budget`。

其他歧义型传输失败仍然映射为 `OutcomeUnknown`，绝不重放。

### R4 — webhook ingress 尚无 SDK 安全适配（中高）

SDK 没有 webhook 签名、重放保护或 Axum extractor。这是有意的边界，但文档和示例不能暗示“开启 dashboard webhook 就等同可靠交付”。在没有签名/重放协议的情况下，推荐 polling 为权威状态源，webhook 只能作为提示。

### R5 — provider 限流行为：尝试取证失败，429 路径仍为 mock-only（中）

本地 QPS 只按请求 start 限制。本轮尝试用受控探测获取真实 429 证据，**未能触发**：

| 探测 | 方式 | 结果 |
|---|---|---|
| 60 并发 | 独立 curl 进程 | 60x 200，无 429 |
| 120 并发（计时） | 独立 curl 进程 | 120x 200，无 429（8.6 rps 为本机 TLS 开销假象，**不是**供应商吞吐） |
| 200 请求 / 64 并发 | SDK（连接池复用） | 200x 200，无 429，**实测 91.9 rps** |

两点结论：

1. **curl 的 8.6 rps 是本地假象。** 每个 curl 进程各做一次 TLS 握手，瓶颈在本机而非供应商。改用 SDK 的连接池后达到 91.9 rps —— 说明用 per-process curl 测吞吐会严重低估，后续压测必须走 SDK 路径。
2. **文档所称 32 rps 在本次条件下未被强制执行。** 200 个请求以约 92 rps 发出，全部 200，没有任何 429。

这**不能**证明不存在限流：可能是按分钟窗口、按端点、按账号等级，或按失败率触发。只能说在单客户端、只读端点、约 92 rps、200 请求的条件下没有观察到。

因此：

- 成功响应中**不存在任何限流元数据**（实测 header 全集里没有 `X-RateLimit-*`，也没有 `Retry-After`）；
- SDK 的 429 解析与 `Retry-After` 退避**至今只有 mock 覆盖**，没有任何真实供应商响应验证过；
- `max_in_flight_duration()` 里 `max_retry_after`(30s) 一项**必须保持保守**，不能因为这次没观察到 429 就调小；
- 示例中的 `max_requests_per_second(32)` 应理解为**自我约束的礼貌限流**，而非镜像供应商的强制上限 —— 这一点现在有实测支持。

**本轮在该账号上的实际消耗：** 三次探测合计 **380 个只读请求**，峰值约 92 rps，全部 200，无 429、无扣费。探测到此为止，不再加压。

**默认值风险（值得单独记住）：** SDK 的 `max_requests_per_second` 默认是 `None`，即**默认不限速**；实测单进程无节流可达约 92 rps。示例里的 32 是显式 opt-in 的自我约束。若消费方不主动配置，单进程即可对单账号打出约 92 rps，10 个实例约 920 rps —— 而供应商文档称 32 rps。本次没观察到强制执行，不代表其他维度（按分钟、按端点、按失败率）不存在；一旦存在，问题会在生产暴露。

**剩余验收要求：** 若要把 429 路径从 mock-only 升级为已验证，需要供应商侧确认限流策略，或在获授权的压测窗口内复现；不应通过持续超发来试探生产账号。

### R6 — key rotation 和运行时凭据生命周期在示例外（中）

SDK 默认不打印 key，但没有 key version、热轮换、撤销传播或 secret manager 集成。生产应用必须把 key 放在 secret manager，轮换时建立新 client、排空旧 client，并保存无敏感值的切换审计；官方资料说明更新 API key 会立即使旧 key 失效。

来源： [SMSPool API key update article](https://www.smspool.net/article/how-to-update-your-api-key-a808e941955c)。

### R7 — observability 仍是 consuming application 责任（中）

当前 tracing 只提供低基数 debug 事件；metrics、告警、订单对账 runbook、429/5xx 预算、数据库连接池和 shutdown SLO 没有内置实现。OPS-001 不能仅由仓库测试通过来激活。

## 4. 后续实施顺序

本轮已先完成 P0-2：`ClaimedOrder` 现在携带持久化状态，`release_for_retry` 会保留 `reconcile_only`，并由真实 PostgreSQL ignored recovery test 覆盖。它只解决状态降级问题，不等于 intent/order 原子关联或租约续期已经完成。

### P0：先修一致性

1. ~~加入 `record_purchase_for_intent` 原子事务 API 和显式关联字段/唯一约束。~~ 已完成基础实现；仍需崩溃/断连注入。
2. ~~`ClaimedOrder` 保留原状态，普通 read-only retry 不得把 `reconcile_only` 降级成 `polling`。~~ 已完成。
3. ~~增加 lease margin。~~ 已完成构造期强制约束（`min_lease` + `max_in_flight_duration`）；仍需请求期 renew 与慢响应/崩溃注入测试。

### P1：补齐协议漂移防线

1. 添加 numeric status synthetic fixtures 和稳定 terminal predicates。
2. 将官方 webhook 资料纳入 contract watch，但在签名和重复语义未证实前只提供 consuming-app adapter 文档。
3. 为 429、失败请求突增、QPS admission timeout 增加 metrics interface；默认不把 provider 300/s 文章硬编码成限额。

### P2：外部证据

1. 只读 live smoke：GET/POST/form/query 变体、active/check、429 header、未知 status。
2. 独立低余额/隔离账号的 paid mutation：intent、响应丢失、active/history 对账、取消时间锁和退款。
3. 类生产 Axum+PostgreSQL：双 worker、慢响应、进程 kill/restart、数据库断连、shutdown 和告警。
4. 受控 pilot：预算、调用量、停止条件、未解决 reconciliation 数量必须为零。

## 5. 研究完成的定义

本研究阶段只有在以下条件满足时才可关闭：

- 以上 R1/R2 有实现或明确的 consuming-app 约束，并有失败注入测试；
- webhook、状态码、GET/POST 和限流矛盾均有“已验证/未验证”标记；
- `production` profile 仍对缺少 LIVE/OPS/PILOT 证据 fail closed；
- 所有 live 证据不含 API key、号码、短信正文、完整订单 ID、绝对余额；
- 文档中的官方事实带来源链接，仓库实现不会把文章描述升级成永久协议保证。
