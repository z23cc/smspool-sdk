# API contract

本文档记录 SDK 实现所依据的证据及其可信边界。它不是 SMSPool 官方服务等级承诺，也不把 Postman 样例推断冒充为线上运行时证明。

## Contract provenance

| 项目 | 值 |
|---|---|
| 源文件 | `postman.json` |
| Collection | `SMSPool API` |
| Postman ID | `b2f10c80-0e84-45b9-a156-ebe58326e6b6` |
| Schema | Postman Collection v2.1.0 |
| Base host | `api.smspool.net` |
| 生成器 | `scripts/postman_contract.py` |
| 当前语义指纹 | `c06cea363b24afd2dd8e3d564e543fe7fdc0610326d60b1102a9acbcadb3db7e` |

语义指纹根据规范化的操作、请求字段和响应形态生成，不使用原文件字节哈希。描述或缩进变化不会制造无意义漂移，协议字段变化会反映在基线 diff 中。

## Verified collection facts

这里的“Verified”只表示已由当前 Postman 文件和审计脚本确认，不等同于所有 endpoint 的线上证明。选定核心 SMS 流程另有脱敏人工观察，见下方 `Live observations`；它不改变 collection contract，也不满足 `LIVE-001` manual gate。

| 项目 | 当前值 |
|---|---:|
| 操作数 | 60 |
| POST | 57 |
| GET | 3 |
| `formdata` body | 57 |
| `urlencoded` body | 2 |
| 无 body | 1 |
| 响应样例 | 103 |
| 无响应样例操作 | 5 |
| 无法解析的 JSON 响应样例 | 0 |

分组数量：Informative 7、SMS 15、Preorders 5、Rental 14、Pricing 2、Carrier 1、Business 4、eSIMs 8、Voucher 4。完整逐操作清单由脚本生成到 [`generated/endpoint-matrix.md`](generated/endpoint-matrix.md)。

### 已证实的协议异常

1. `GET /country/retrieve_all` 和 `GET /service/retrieve_all` 在 Postman 中携带 `formdata` body；当前 descriptor 和 transport 忠实发送 multipart GET，服务端实际是否更适合 query 仍等待 `LIVE-001`。
2. `GET /business/users` 的 raw URL 缺少 `https://`，结构化 path 仍为 `/business/users`。
3. Voucher 的单个生成与批量生成共用 `POST /voucher/generate`，由请求字段区分。
4. `/request/areacodes` 和四个 Voucher 操作缺少响应样例，因此不能直接声明稳定强类型返回值。
5. 受控 live 观察显示 `POST /sms/all_stock` 的响应超过 1 MiB 与 16 MiB；SDK 不再尝试将该响应作为 `Vec<AllStockEntry>` 缓冲，调用会 fail-closed 并建议使用有界 `sms/stock`。
6. 集合中存在字符串金额、数字金额、以分为单位的字段、`0/1` 布尔、数字/字符串状态以及双重编码 JSON。
7. 至少存在 HTTP 200 且顶层 `success: 0` 的业务失败样例；成功与否不能只看 HTTP 状态。
8. 部分成功响应没有 `success` 字段，部分响应直接返回数组。

## Request contract

### Method and encoding

`src/endpoint.rs` 为全部 60 个操作保存 method、path、body mode、认证模式和 safety class；`src/transport.rs` 统一执行。当前实现规则：

- `formdata` 使用 multipart 请求；在在线验证证明等价前不得擅自改成 urlencoded。
- `urlencoded` 使用 `application/x-www-form-urlencoded`。
- 两个 collection 中的 multipart GET 当前继续使用 body；只有 `LIVE-001` 的只读证据才能改变该选择。
- 请求字段使用 Postman 原始 wire name；Rust 公共 API 使用一致的 snake_case。公共 request builder 决定是否省略 optional 字段；共享 wire 层不会全局删除空字符串，因此默认 `search=""` 和大小写敏感 `Search=""` 会忠实发送。
- 大小写敏感字段（例如 `Search`）必须显式 rename，不能依赖全局命名规则。

### Authentication

集合同时出现 Bearer auth 和表单 `key`。当前 endpoint descriptor 显式使用以下模式：

- `Bearer`
- `FormKey`
- `BearerAndFormKey`
- `Public`

在真实验证完成前，不把“所有端点都只需要一种认证”写成稳定事实。任何模式下，API key 都不得进入 URL query、错误正文、tracing field 或默认 `Debug`。SDK 禁用 reqwest 自动重定向且不接受外部 client 注入；明文 mock base URL 只允许显式 loopback IP 且强制 `no_proxy()`，防止 307/308 重放 form-key body、环境代理转发或跨 origin 泄露。

### Request validation

本地只拒绝确定无效的输入，例如空 API key、非法分页、零天租期或无法序列化的 area code 列表。价格、库存和服务可用性等服务端业务条件不能伪装成本地静态校验。

## Response contract

### Decode decision order

所有 endpoint 共用以下顶层判定顺序：

1. 读取 HTTP status 和 headers；若为 429，立即映射 `RateLimited`，不检查 `Content-Length`、不读取 body，非幂等操作绝不自动重发。
2. 对其他 status 有界读取 body；超大或停滞响应按 safety class 分类。
3. JSON 解析失败时根据 safety class 判定：ReadOnly 返回脱敏的 `Decode`；Mutation/PaidMutation 在请求可能已送达时返回 `OutcomeUnknown`。
4. 只检查**顶层** `success`；`0`、`false`、`"0"` 视为失败。
5. 非成功 HTTP status 或顶层失败对象统一进入 `ApiError` 分类。
6. 其余内容按 endpoint 类型使用 `serde_path_to_error` 解码；HTTP 成功但 mutation 结果无法解码且无法证明失败时，同样返回 `OutcomeUnknown`。

禁止递归扫描嵌套 `success` 字段，否则内部业务对象可能导致整个响应被误判。

### Type normalization

| Wire 现象 | Rust 设计 |
|---|---|
| `"0.24"` 或 `0.24` 金额 | `Money(Decimal)` |
| `cost_in_cents: 24` | 独立 `Cents(u64)`，不得与 Money 猜单位 |
| 手机号为数字或字符串 | 宽容解码为脱敏 `PhoneNumber`；读取明文必须显式 `expose()` |
| `0/1`、布尔或字符串布尔 | `lenient_bool` |
| 数字、字符串、布尔或其他状态 | `StatusValue` 保留原始类别；SMS/Preorder 只按已观察到的结构分类 |
| Unix 秒、相对秒、日期字符串 | 独立 newtype，不混用单位 |
| JSON 字符串内再次编码 JSON | 可保留原文的 `DecodedJson<T>` |
| 只有空数组样例 | 暂用不稳定/原始类型，获得证据后升级 |

未知响应字段默认忽略；未知枚举值必须保留，不能 panic。错误对象保留有界 raw value 供排障，但默认日志输出必须脱敏。typed response 中 `Value` fallback 使用 `RedactedValue`，`StatusValue::Other` 也隐藏内部 JSON；只有显式 `expose()` 才返回原值。

## Reliability classification

全部 endpoint descriptor 已显式声明以下类别之一：

- `ReadOnly`：目录、余额、价格、库存、查询、历史；可在受限条件下重试。
- `Mutation`：取消、激活、更新等；没有在线幂等证据时不自动重试。
- `PaidMutation`：购买、付费查询、续期、充值、生成 Voucher；绝不自动重试。

非幂等请求发出后遇到超时、连接重置或响应损坏时不会被当作普通失败重试。当前 transport 返回脱敏 `OutcomeUnknown`，包含 endpoint、safety class、阶段、可选 HTTP status 和静态对账提示。发送前可证明的连接失败仍返回普通 transport error。

## Live observations (bounded)

`acceptance/live-observations.json` 记录一份由操作员提供的脱敏观察，而不是可验证的 gate attestation。观察到：Poland / GMX / pool 3 的 USD 0.02 报价与购买、初次 check pending、首次 cancel 的显式大于 60 秒时间锁、后续 cancel 成功，以及 USD 0.02 的余额差额；`sms/all_stock` 超过两个响应上限。记录明确省略 API key、号码、短信/验证码、完整订单 ID 和绝对余额。

这些观察只证明该次受控流程的解码与业务结果，不能证明 mutation 幂等、短信送达、账号级限额、全部 endpoint 认证组合、生产告警、持久化恢复或 pilot。余额差额可能受并发账户活动影响，且精确时间锁字符串未保留。LIVE/OPS/PILOT gates 仍保持 pending。

## Implemented surface and workflow boundary

稳定入口为 `client.catalog()`、`client.pricing()` 和核心 `client.sms()`（purchase、check、active、cancel、history）。其余 SMS、Preorder、Rental、Carrier、Business、eSIM、Voucher 和通用 raw 请求必须经 `client.experimental()` 访问。`/request/areacodes` 与四个无响应样例的 Voucher 操作返回有界 `serde_json::Value`，不伪造强类型成功契约。

`SmsCheck` 只按字段存在性分为 Pending、Received、Terminated，不解释 vendor status 数字含义；Preorder 同样按 `order_code`/`phonenumber` 等已观察字段区分形态。未知字段被忽略，未知 status 原值被保留。

`src/poll.rs` 只调用稳定的 read-only `sms.check` 和 `sms.active`。单订单 polling 使用绝对 deadline、取消、间隔上限、jitter 和 `Retry-After` floor；验证码只能由调用方 extractor 产生。`ActiveOrdersWatcher` 顺序轮询并按 status/code/full-code 去重，在更新映射前检查最大跟踪数量，active snapshot 中消失的订单会从内存映射移除。所有 workflow 状态都非持久化，不能替代生产任务或重启恢复。

核心实现、fixture 和 mock 测试不发起真实 SMSPool 请求；live 观察是单独的、不可复现的人工记录，不会在普通测试中重放付费操作。

## Cancellation, reconciliation, and QPS controls

取消是唯一提供高层重试策略的 mutation 工作流。`cancel_with_reconciliation` 默认只发送一次 `sms/cancel`；只有调用方用 `CancelTimeLockRule::message` 或 `machine_type` 配置**完整、精确**的已知时间锁签名时，才会等待并发送下一次取消。`max_cancel_attempts` 和 `max_outcome_unknown_reconciliation_checks` 都是必填的正数上限。每个已发出的取消尝试附带一次即时对账（该对账用于发现短信已送达并停止继续取消，因此只受 `max_cancel_attempts` 约束）；`max_outcome_unknown_reconciliation_checks` 仅限制 `OutcomeUnknown` 之后的连续只读对账轮次。对账总次数上限为二者之和，每次最多 3 个只读请求。收到 `OutcomeUnknown` 时永远不再发送取消，只执行有限的 `sms/check`、`sms/active` 和可选余额差额观察。

### `sms/check` 与 `request/active` 的分工（实测）

供应商在 `sms/check` 的响应里附带（该字段不在任何 Postman 样例中）：

```json
"warning": "For high volume requests we recommend using /request/active instead of /sms/check"
```

但这**不是简单替换**，两者能力不同，实测差异如下：

| 能力 | `sms/check` | `request/active` |
|---|---|---|
| 请求数 | 每订单 1 次（N 单 = N 次） | 每轮 1 次，与订单数无关 |
| 携带短信内容 | 是（`sms` / `full_sms`） | 是（`code` / `full_code`） |
| 已退款/已取消订单 | 明确返回 `status: 6` + `message` | **直接从列表消失** |
| 区分「已退款」与「从未存在」 | 可以 | **不能** |

关键实测：一笔订单退款后，`request/active` 的条目数由 5 降为 4，该订单**无声消失**；同一时刻 `sms/check` 仍明确返回 `status: 6, message: "This order has been refunded"`。

因此结论是**分工而非替换**：

- **批量探测用 `request/active`** —— 这是供应商建议的方向，也是请求数从 O(N) 降到 O(1) 的唯一途径；
- **终态判定仍需 `sms/check`** —— 订单从 active 消失时，只有它能说清是退款、取消还是过期；
- **不得把「从 active 消失」单独当作终态信号**。`ActiveOrdersWatcher` 的文档已明确它不是持久化完成追踪；`cancel_with_reconciliation` 也因此要求 `Absent` 与 `sms/check` 同时佐证。

`examples/axum.rs` 的 worker 按此实现：每轮取一次 active 快照，仅把「明确仍在 pending」的订单走快路径跳过 `sms/check`；其余一律回落到 `sms/check` 取权威答案。该启发式是单向的 —— 判错只会多查一次，不会产生错误的终态。快照请求失败时自动退化为逐单查询。

终态判定需要交叉佐证：`SmsCheck` 的 `Terminated` 变体仅由 `message` 字段的存在推断，而该供应商也会在非终态响应里返回人话。因此时间锁分支只有在 `request/active` **明确报告订单已不存在（Absent）** 时才停止取消；`Present` 表示两个观察互相矛盾，`Unavailable`（含解码失败）表示无法确认，两者都会继续有界重试，以免把仍然存活的号码当成已结算而放弃退款。`Received` 由真实短信内容支撑，无需佐证即可结束。

余额观察返回 `SignedMoneyDelta`，不会暴露或持久化 before/after 绝对余额；差额与 expected refund 相等只表示辅助证据，不能解决并发账户活动或供应商归因问题。取消 mutation 发出后，调用方不能依赖取消 token 让请求中途消失，应依靠结果 disposition 或持久化 `reconcile_only` 状态完成后续对账。

`ClientBuilder::max_requests_per_second` 是可选的客户端 start-rate limiter，与 `max_concurrency` 独立，跨 `Client` clone 共享，并对 read-only retry 的每次尝试生效。QPS 等待有单独的 `TimeoutPhase::QpsAdmission`；未配置时 SDK 不宣称或猜测账号限额。`sms/all_stock` 的真实响应超过当前安全缓冲上限，因此公开方法 fail-closed 返回 `UnsupportedOperation`，请按国家/服务/pool 使用有界 `sms/stock`。

## Unverified assumptions

以下事项在获得真实、只读或受控测试证据前必须保持未验证状态：

- multipart 与 urlencoded 是否可互换。
- 各 endpoint 对 Bearer 和表单 `key` 的真实要求。
- `quantity > 1` 的购买响应形态。
- `create_token=1` 的长期稳定返回结构。
- `web=1` 对 pool 返回形态的完整影响。
- 429 是否稳定提供 `Retry-After`，以及账号级实际限额；SDK 的 `max_requests_per_second` 只是客户端保护，不是供应商限额证明。
- Unix 时间字段的单位、无时区日期字符串的时区。
- cancel、resend、activate 等 mutation 是否服务端幂等；取消工作流只在调用方配置精确时间锁签名后重试，不宣称供应商幂等。
- 所有无样例 endpoint 的成功与失败形态。
- SMSPool 对请求去重或客户端幂等键的支持。

## Contract change procedure

修改 `postman.json` 后必须执行：

```bash
python3 scripts/postman_contract.py generate
python3 scripts/postman_contract.py check
```

随后人工审查以下 diff：

- `contracts/postman-baseline.json`
- `docs/generated/endpoint-matrix.md`
- 新增、删除或改变的请求字段和响应 shape
- known warnings 的增减

生成命令只更新证据，不代表审查通过。baseline 递归记录对象字段、字段必选性、数组元素、标量类型和双重编码 JSON 的内部 shape；工具回归测试保证嵌套类型变化会改变契约指纹。协议变化需要同步响应类型、fixture、wiremock 测试和稳定性说明；不能通过重新生成基线掩盖不兼容变更。
