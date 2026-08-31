# Production acceptance standard

本文档定义“文档基础完成”“SDK 可测试”和“允许生产试运行”三个不同层级。验收脚本遵循 fail-closed：未实现、未激活或必须人工确认的 gate 都不会被静默跳过。当前已补充一份独立的脱敏 live 观察记录和 PostgreSQL 恢复演练入口，但两者都不会自动激活 manual gates。

机器可读事实来源为 [`../acceptance/gates.json`](../acceptance/gates.json)；本文解释其判定语义。二者不一致时必须修复，不能仅修改脚本获得绿色结果。

## Acceptance profiles

### `foundation`

用于验证 contract-first 基线和仓库工具。要求：

- gate 定义合法；
- foundation 工具自身的回归测试通过；
- 必需文档存在且包含关键章节；
- Postman 可解析、没有阻断错误；
- 生成的 baseline 和 endpoint matrix 与 Postman 完全一致。

执行：

```bash
./scripts/acceptance.sh foundation
```

### `sdk`

当前已激活的仓库完成度 profile。它包含 foundation，并增加 fmt、all-target/all-feature Clippy、单元/集成测试、全部 103 个 fixture 解码、wire contract 和脱敏测试。该 profile 完全离线，不需要或读取 `SMSPOOL_API_KEY`。

```bash
./scripts/acceptance.sh sdk
```

### `production`

包含 sdk 全部门槛，并要求显式 live smoke、运行环境检查和受控 pilot。manual gate 必须由负责人记录证据，不能用空脚本替代。激活后的 manual gate 通过 `acceptance/evidence.json` 验证当前 Git revision、契约指纹、gate 定义哈希、审批人、证据引用和有效期；缺失或过期均失败。

```bash
./scripts/acceptance.sh production
```

## Exit semantics

- `0`：所选 profile 中所有 gate 都已激活且通过。
- `1`：至少一个 gate 失败、pending 或需要人工证据。
- `2`：验收定义、JSON 或命令使用本身无效。

`foundation` 通过不代表 SDK 已实现；`sdk` 通过不代表实际账号、限流和运维恢复已验证。

## Gate catalogue

| Gate | 当前状态 | 类型 | 通过标准 |
|---|---|---|---|
| META-001 | active | automated | `acceptance/gates.json` schema、ID、profile 引用合法 |
| TOOL-001 | active | automated | shape 漂移、清理边界和 gate schema 回归测试通过 |
| DOC-001 | active | automated | 必需文档、基线和关键章节存在且非空 |
| CONTRACT-001 | active | automated | Postman 无阻断错误，确定性产物无漂移 |
| RUST-001 | active | automated | `cargo fmt --all -- --check` 通过 |
| RUST-002 | active | automated | all targets/all features Clippy 在 `-D warnings` 下通过 |
| RUST-003 | active | automated | `cargo test --all-features` 通过 |
| FIXTURE-001 | active | automated | 103/103 fixture 解码，并对 60/60 descriptor/request wire 校验 baseline method/path/body mode/inherited Bearer/active field names 与确定性 values；`sms/all_stock` 的本地拒绝是有意行为 |
| WIRE-001 | active | automated | method/path/body/auth、200+失败、429、retry 和 OutcomeUnknown mock 测试通过 |
| SEC-001 | active | automated | Debug/tracing 不泄露凭据和客户数据 |
| LIVE-001 | pending | manual | 明确 opt-in 的只读线上 smoke 通过且保存脱敏证据 |
| OPS-001 | pending | manual | 指标、告警、并发、持久化轮询和重启恢复在类生产环境验证 |
| PILOT-001 | pending | manual | 受控生产 pilot 完成且没有未解决对账异常 |

## Activation rule

把 gate 从 `pending` 改成 `active` 前必须同时满足：

1. 对应实现和测试目标已经存在；
2. 命令在干净 checkout 中成功执行；
3. 失败场景确实会令命令非零退出；
4. 文档说明其覆盖范围和未覆盖范围；
5. 对付费或线上行为没有隐式触发。

禁止仅因为“暂时不想阻塞 CI”删除 gate。暂不要求的能力保留 pending，并使用合适 profile。

## Contract acceptance

`CONTRACT-001` 执行：

```bash
python3 scripts/postman_contract.py check
```

阻断条件包括：

- Collection JSON 或 schema 不可解释；
- leaf 缺少 method/path；
- 不支持的 body mode；
- response 样例声称为 JSON 但无法解析；
- 已生成 baseline 或 matrix 与源文件不一致。

已知但可继续推进的异常进入 `known_warnings`。warning 必须在 diff 中可见，不等于已经在线验证。

## Rust quality acceptance

当前已激活的 SDK gate 覆盖：

- 默认 feature 与 all-features 构建；
- 最低支持 Rust 版本构建；
- serde 宽容类型和未知枚举；
- HTTP 200 + 顶层失败对象；
- 非 JSON、超大 body、字段路径解码错误；
- 全部 60 个公共操作的 method/path/body mode、active field names/values；默认空 `search` 与大小写敏感空 `Search` 不被全局过滤；
- Bearer/FormKey/组合认证且日志脱敏；
- 禁用自动 redirect、拒绝外部 client 注入、HTTP mock 限 loopback IP 且强制绕过环境代理；
- ReadOnly 限定重试；
- PaidMutation 从不自动重试；
- mutation transport 异常返回 OutcomeUnknown；
- 429 从 headers 立即分类且不读取超大/停滞 body；
- 取消、deadline、并发 permit 和 429 退避；queued watcher event 也不能越过取消/deadline；
- caller-supplied code extraction、active watcher 去重/移除/容量上限；
- public API doctest 和 Axum 示例编译。

Rust 1.85 库级锁定依赖检查与显式 example 检查也属于发布前完整验证：

```bash
cargo +1.85.0 check --locked --lib --features tracing
cargo check --examples --all-features
cargo test --test workflow
```

fixture 数量必须与提取 manifest 匹配，不能只抽样验证容易通过的响应。

## Live-test safety

任何真实 SMSPool 测试默认关闭。当前 [`acceptance/live-observations.json`](../acceptance/live-observations.json) 是操作员提供的、不可复现的脱敏观察；它记录了选定核心流程，但 `gate_eligible` 明确为 `false`，不具备 revision、审批人和不可变外部引用，因此不能替代 `acceptance/evidence.json`。

已记录的有限事实：Poland / GMX / pool 3 的 USD 0.02 报价与一次购买成功、初次 check pending、首次 cancel 返回大于 60 秒时间锁、后续 cancel 解码成功、观察到 USD 0.02 余额差额；`sms/all_stock` 超过 1 MiB 与 16 MiB。未记录 API key、号码、短信/验证码、完整订单 ID 和绝对余额。余额差额可能受并发账户活动影响，精确时间锁签名也未保留。

`LIVE-001` 的未来正式脚本仍必须满足：

- 同时要求显式命令行开关和 `SMSPOOL_API_KEY`；
- 默认只调用事先确认的只读/无扣费端点；
- 启动时打印将访问的 endpoint 清单；
- 不在输出中打印 key、号码、短信或完整响应；
- 具有短 timeout 和严格调用次数上限；
- 付费测试使用单独命令、预算上限和人工确认，不属于常规 CI。

没有凭据时应报告“未执行”，不能把 live gate 记为通过。人工 gate 完成后可生成与当前仓库绑定的证明模板：

```bash
python3 scripts/acceptance.py evidence-template LIVE-001 > acceptance/evidence.json
```

模板只能在干净且已有提交的 checkout 中生成；其中审批人和不可变证据引用必须替换。`acceptance/evidence.json` 是环境特定文件，默认不提交 Git。证据过期、revision/契约/gate 定义变化后都必须重新验收。

## Operational acceptance

仓库提供可运行的 Axum + PostgreSQL 示例和一个显式 ignored 的恢复测试，但生产 Axum 服务而非 SDK 仓库负责提供以下证据：

```bash
# 编译示例（不连接数据库、不发起 provider 请求）
cargo check --example axum --features postgres-example
# 使用真实 PostgreSQL 演练 claim/租约/重启恢复；失败或缺少环境时不会伪造 OPS 通过
DATABASE_URL=postgres://... SMSPOOL_ORDER_KEY=$(openssl rand -hex 32) \\
  cargo test --features postgres-example --test postgres_recovery -- --ignored --nocapture
```

该测试使用真实 sqlx/PostgreSQL 表、事务、`FOR UPDATE SKIP LOCKED`、租约过期和重新 claim，并把 provider check 请求指向本地 scripted server；它验证数据层恢复语义，不验证供应商线上可用性、生产容量、告警或 shutdown 压力。

本次仓库演练记录（2026-08-29）：`postgres:16-alpine` 一次性容器中 `postgres_claim_restart_and_read_only_recovery` 以 `--ignored` 运行，1/1 通过；随后 `target/debug/examples/axum` 使用同一 PostgreSQL 数据库启动两次，`/healthz` 两次返回 `{"status":"ok"}`，每次均以 SIGINT 优雅退出。该记录不含数据库 URL、加密 key 或 provider 数据，仍不激活 `OPS-001`。

- 订单及轮询状态持久化，进程重启后可恢复（示例仅保存加密 provider order ID、不可逆指纹和非敏感元数据；付费请求前应先记录 intent，`OutcomeUnknown` 只进入 `reconcile_only`）；
- 全局并发上限和批量 active polling；
- 429、供应商 5xx、timeout、decode drift、OutcomeUnknown 的指标和告警；
- OutcomeUnknown 对账 runbook；
- API key 轮换和撤销流程；
- 日志/trace/错误聚合平台中的敏感数据抽查；
- shutdown 时停止接单、保存状态并取消后台任务；
- 供应商不可用时的用户可见降级策略。

## Production definition of done

允许核心 SMS 流程进入受控生产 pilot 前，必须满足：

- `foundation`、`sdk` 所有 gate 通过；
- LIVE-001 有时间、版本、账号环境和脱敏结果记录；
- 核心请求编码和认证经过真实验证；当前 live 观察仅覆盖选定核心路径，不能覆盖全部 endpoint；
- 至少演练一次下单响应丢失后的 OutcomeUnknown 对账；
- 至少演练一次进程重启后的轮询恢复；仓库测试命令必须在真实 PostgreSQL 环境显式运行并保存操作证据；
- 并发/429 压力测试没有无界任务或请求风暴；
- pilot 有请求量、金额和停止条件；
- experimental endpoint 不被生产关键路径依赖。

pilot 完成且遗留问题关闭后，PILOT-001 才能通过。crate 版本号本身不构成生产证据。

## Maintenance commands

```bash
# 验证 gate 定义
python3 scripts/acceptance.py validate

# 检查文档
python3 scripts/acceptance.py check-docs

# 列出 profile 状态
python3 scripts/acceptance.py list

# 为已激活的人工 gate 生成当前证明模板
python3 scripts/acceptance.py evidence-template LIVE-001

# 当前仓库完整 SDK 检查（纯离线）
python3 scripts/acceptance.py run sdk

# 可选：检查脱敏 live 观察 schema（不会激活 LIVE-001）
python3 scripts/acceptance.py validate
```
