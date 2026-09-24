# IMS 命名迁移第二阶段：错误码、HTTP 契约与 JSON 字段

> 本文是第二阶段的执行依据。第一阶段（模块路径、内部类型、规范路由）已完成，
> 记录在 `docs/IMS_NAMING_MIGRATION.md`。
>
> 开工基线：`36a0a2e`，分支 `dev/1.1.5-modem-backends`，CI 全绿。
> 每完成一项请在本文勾选，并在"变更记录"追加提交号。

## 1. 背景与语义纠正

早期把 "VoLTE" 当作 IMS 注册的同义词。实际上 VoLTE 只是 IMS 之上的语音承载：
短信（SMSoIP）、语音、补充业务共用**同一条 IMS 注册**，"打开 VoLTE" 并不等于
建立 IMS 注册。因此 `volte_*` 作为注册链路的命名是错的，应为 `cellular_ims_*`。

第一阶段已纠正代码内部语义。本阶段处理表层契约。

## 2. 关键前置结论（已核实，非估算）

### 2.1 数据库不持久化 IMS 错误码

`IMS_NAMING_MIGRATION.md` 曾记载错误码 "stored in the database"。经逐列核实，
**该结论不成立**。不能把其他业务的错误记录或内存态字段当作 IMS 注册错误码列：

| 列 | 所属表 | 实际内容 |
|---|---|---|
| `last_error` | `vowifi_soak_runs` | VoWiFi 长稳测试 |
| `last_error` | `notification_queue` | 通知发送失败原因（如 HTTP 状态） |
| `failure_code` | 通话记录 | 通话失败码，独立词表 |
| `last_failure_reason` | VoWiFi 内存态 refresh 管理器（非数据库列） | `vowifi_refresh_rebuild_pending` 等 |

`volte_refresh_stats` 表名含 volte，但只存 `refresh_count` / `last_refresh_at` /
`updated_at`。`CellularImsStatus.last_error` 是内存态，经 API 暴露，不落盘。
通知正文与系统事件均不携带 IMS 错误码。

**结论：错误码改名不存在历史数据兼容问题，唯一消费者是前端。**

### 2.2 持久化名称以迁移处理

2026-09-23 已纳入步骤 7：`volte_refresh_stats` 与 transport/event/MT 标记带迁移更新。
`volte_enabled` 实为线路配置 JSON 键，不是 SQL 列；由步骤 5 的 serde alias 兼容读取。
`carrier_Bundles` 的 LTE/NR 注册字段本来就是 `lte_ims_status` / `nr_ims_status`。

### 2.3 RustRover 不适用于本次重构

两点原因：一是 RustRover MCP 当前无法连接（`Connection closed`）；二是即使连通，
这些码是**字符串字面量**而非符号，IDE 的 rename refactoring 不处理字面量内容。
可用手段只有全局文本替换，ripgrep 已足够。真正的风险在前端子串匹配（见 4.2），
需要靠守卫测试而非 IDE 解决。

## 3. 精确清点

### 3.1 错误码

重新逐行判定 `#[cfg(test)]` 归属后的准确数字（此前按“首个 `#[cfg(test)]`
之后即测试”统计是错的，`live.rs` 有 4 处 `#[cfg(test)]`，`channel.rs` 有内联用法）：

- 生产字面量点 **270** 处，测试专用 **56** 处。
- `errors.rs` 已有 **70** 个 `code::*` 常量与 `verr!` 宏，**步骤 1 的“散落字面量”
  前提对该文件不成立**；真正缺口是调用方绕过常量。
- 去掉非错误码项后，生产代码中 **86 个码值需要新建常量**，仅 5 个已有常量可复用。
- 另有 8 处 `format!` 前缀式码（如 `volte_register_refresh_retry:{}`），
  需拆成常量 + 后缀。

- 后端分布（按文件，生产/测试）：

| 文件 | 生产 | 测试 |
|---|---|---|
| `cellular_ims/live.rs` | 84 | 2 |
| `cellular_ims/errors.rs` | 70 | 5 |
| `cellular_ims/channel.rs` | 61 | 1 |
| `api/handlers.rs` | 25 | 6 |
| `cellular_ims/pcscf.rs` | 6 | 0 |
| `cellular_ims/sip.rs` / `cellular_ims/bearer.rs` | 各 4 | 0 |
| `platform/config.rs` | 0 | 23 |

`channel.rs` 完全没有引用常量模块，61 处全是裸字面量，是本步最大单点。
`config.rs` 的 23 处全在测试内（键迁移断言表），属预期，不改。

- 旧分布表（首次统计，保留以说明修正过程）：

| 文件 | 处数 |
|---|---|
| `connectivity/modems/ims/cellular_ims/live.rs` | 81 |
| `connectivity/modems/ims/cellular_ims/errors.rs` | 71 |
| `connectivity/modems/ims/cellular_ims/channel.rs` | 62 |
| `platform/config.rs` | 23 |
| `api/handlers.rs` | 23 |
| `connectivity/modems/ims/cellular_ims/pcscf.rs` | 6 |
| `platform/db.rs` | 5 |
| `services/system/diagnostic_log.rs` / `main.rs` / `cellular_ims/sip.rs` / `cellular_ims/bearer.rs` | 各 4 |
| `connectivity/core/register.rs` | 3 |

- 另有 20 个 Rust 测试文件对码字面量做断言。
- 注：`connectivity/core/ims_failure.rs` 含 **0** 处 `volte`，其词表已是中性命名。

### 3.2 配置键 / serde 字段

6 个键**已有** `rename = "volte_*"` + `alias = "cellular_ims_*"`，即新名已可读入，
仅写出仍用旧名：

- `volte_connection_enabled`、`volte_auto_restore`、`volte_profile_selection`
- `volte_ip_families`、`volte_ip_families_auto`、`volte_enabled`

3 个键**尚无** alias，需补：

- `volte_profiles`（`api/models.rs:142,184`）
- `volte_ims`（`api/models.rs:1246`）
- `volte_ready`（`vowifi/profile_store.rs:1242,1261`）

### 3.3 前端

`volte_*` 引用分布在 9 个文件，集中在错误码格式化：

| 文件 | 处数 |
|---|---|
| `pages/sim/cellularImsErrorFormat.ts` | 28 |
| `api/contracts.ts` | 10 |
| `pages/sim/ModemLinesPanel.tsx` | 9 |
| `pages/sim/CellularImsProfileDialog.tsx` | 4 |
| `pages/sim/LineRuntimeDetails.tsx` | 3 |
| `pages/sim/CarrierProfilesPanel.tsx` | 3 |
| `pages/SMS.tsx` / `pages/SimCard.tsx` / `pages/phone/VoiceRoutingPanel.tsx` | 各 1 |

### 3.4 必须排除的持久化值（新增约束）

`volte_ims` **不是错误码，而是短信 transport 值**，会写入 `sms_messages.transport`
列（另有 `vowifi_ims` / `modem` / `trunk`）。前端 `pages/SMS.tsx:97` 与
`pages/sim/LineRuntimeDetails.tsx:127,179` 按精确值匹配。

不能仅靠文本替换改名，否则历史短信会显示错误来源；后续授权已由步骤 7 带迁移处理，
后端与前端仍读取旧值。本节保留其“非错误码”的分类，不再表示延期。

同类需排除的还有断连原因串（经 `disconnect_live_for_line` 传入，非错误码）：

- `volte_ip_families_changed`（`handlers.rs:8766`）
- `volte_profile_selection_changed`（`handlers.rs:8583`）
- `volte_line_connection_disabled`（`handlers.rs:8694,8805`）

这些可改，但属于独立语义类别，需与错误码分开处理，避免混入同一张映射表。

### 3.5 其他消费者

- `bruno-api/`：9 个文件引用 volte（含 `test_volte_status.bru`、`get_volte_line.bru` 等）。
- `.github/scripts/test_ims_fallback_boundary.py`：2 处。

## 4. 风险

### 4.1 HTTP 路由与 JSON 字段

`/cellular-ims/*` 规范路由已存在，`/volte/*` 作为别名**长期保留不删除**，
已有别名回归测试 `cellular_ims_route_aliases_keep_auth_and_response_contract`。
JSON 字段改名必须前后端同步发布，否则页面读不到字段而静默显示空值。

### 4.2 前端子串匹配错位（本阶段最高风险）

清点阶段曾将 5 组配置键/断连原因误称为“错误码子串”：

- `volte_ip_families` ⊂ `volte_ip_families_auto` / `_changed` / `_duplicate` / `_empty`
- `volte_profile_selection` ⊂ `volte_profile_selection_changed`

它们不是最终 `errors::code` 表的码值；158 项表中无互为子串的码。旧前端仍有意依赖
前缀族/子串匹配，所以步骤 2–3 改为精确 token 匹配并用守卫防止回归：

| 前缀 | 覆盖后端码数 |
|---|---|
| `volte_digest_` | 6 |
| `volte_ipsec_` | 3 |
| `volte_bearer_netdev_` | 3 |
| `volte_at_` | 2 |

若改名后产生新的子串包含关系，匹配会**静默错位**：不报错，但提示信息给错。
编译器和现有测试都抓不到这类缺陷。这是必须先建守卫再改名的理由。

## 5. 执行顺序

顺序不可调换：第 3 步的守卫是第 4 步能否机械化的前提。

### 步骤 1 — 后端错误码收敛为常量

前提更正：`errors.rs` **已有** `pub mod code` 常量模块（70 个常量）与 `verr!` 宏，
并非“散落字面量”。真实缺口是生产代码大量绕过常量直接写字面量：
`channel.rs` 完全未使用常量，`live.rs` 也大量直写。

生产字面量共 189 处、91 个不同值；其中 5 个已有常量，**86 个需新增常量**。
用量最高的几个：`volte_voice_call_unknown`(15)、`volte_channel_local_addr_failed`(14)、
`volte_channel_read_timeout`(10)、`volte_rtp_local_addr_failed`(8)。

- [x] 在 `errors.rs` 的 `code` 模块补齐缺失常量（值保持不变）：70 → **158** 个
- [x] 改引用常量：`channel.rs`(62) / `live.rs`(81) / `handlers.rs`(17) /
      `pcscf.rs`(6) / `sip.rs`(4) / `bearer.rs`(4) / `config.rs`(5)
- [x] 本步**不改名**，只让编译器接管引用正确性
- [x] 导出全量码清单 `code::ALL`（158 项）供守卫读取
- [x] `errors.rs` 内新增两项自检：`ALL` 无重复、无码互为子串

本步完成情况与原计划的差异：

- `format!` 前缀点（8 处）**未**改为常量拼接。这些模板形如
  `volte_baseband_wedged:{error}`，其前缀本身不是 `code` 表中的码；
  强行拆成常量拼接会引入一批只被单点使用的常量，反而降低可读性。
  它们已在守卫中显式排除，改名时按前缀单独处理。
- `connectivity/core/register.rs` 保留 2 处字面量
  （`volte_channel_read_timeout` / `volte_channel_read_retryable`）。
  `connectivity/core` 是 `cellular_ims` 的**下层**，让它引用上层常量会倒置
  分层依赖。已确认 `core` 目前不依赖 `cellular_ims`，故维持字面量并在守卫中排除。
- `config.rs` 的 5 处生产校验码已转换；该文件另有 16 处字面量全在测试模块内
  （serde 迁移断言表），属预期保留。

额外产出（原计划未列，实施中发现必要）：

- [x] 新增 `.github/scripts/test_ims_error_code_table.py`（7 项），断言
      `code::ALL` 与声明集合**完全一致**、无重复值、无子串关系，
      并禁止调用点重新写回字面量
- [x] 该守卫已做负向验证：漏加 `ALL`、引入子串、重复码值、调用点写回字面量
      四种情形均如期失败
- [x] 修正 `test_ims_fallback_boundary.py`：原先断言 `pcscf.rs` 含码字面量，
      改为断言调用点引用常量 **且** 中心表持有该值（约束更强）

### 步骤 2 — 前端改为精确查表

- [x] `cellularImsErrorFormat.ts` 从 `includes()` 改为精确查表：`last_error` 按
      `[a-z][a-z0-9_]*` 切成 token，与码表逐个相等比较；嵌套在 detail 里的码同样命中，
      嵌在更长标识符里的码**不**命中
- [x] 前缀族（`digest_` / `ipsec_` / `bearer_netdev_`）显式列举成员，不再靠前缀；
      `volte_at_` 族在前端并无消费者，无需列举
- [x] 码表来源：新增 `frontend/src/pages/sim/cellularImsErrorCodes.ts`，由
      `.github/scripts/gen_cellular_ims_error_codes.py` 从 `errors.rs` 生成（158 项），
      并导出 `CellularImsErrorCode` 类型，formatter 里写错码名会在 `tsc` 阶段报错
- [x] `旧码 → 新码` 映射：不另建运行时映射表。前后端同包发布，前端永远只需认识
      同版本后端的码；改名由步骤 4 的机械替换一次完成
- [x] 保持对外提示文案不变

### 步骤 3 — 建立一致性守卫

- [x] 新增 `.github/scripts/test_ims_error_code_contract.py`（5 项）：前端码表与
      后端码表**完全一致**、生成文件为最新、formatter 只引用真实存在的码、
      formatter 不得对码做子串匹配、前端码表无子串关系
- [x] 断言无任一码是另一码的子串（Rust 侧与两个 Python 守卫三处同时约束）
- [x] 接入：两个工作流均以 `unittest discover -p 'test_*.py'` 自动发现，无需改过滤器
- [x] 负向验证：多列一个码、漏列一个码、formatter 改回 `includes()`、
      formatter 引用不存在的码——四种情形均如期失败

### 步骤 4 — 统一改名

改名规则：`volte_X` → `cellular_ims_X`；`volte_ims_X` → `cellular_ims_X`（避免
`cellular_ims_ims_`）；`line_volte_connection_disabled` →
`line_cellular_ims_connection_disabled`（常量同名改为
`LINE_CELLULAR_IMS_CONNECTION_DISABLED`）。按标识符边界做 token 精确替换，
映射表只含码表 158 项 + 7 个 `format!` 前缀 + 3 个测试字面量，配置键、
`volte_ims` transport 值与 `volte_refresh_stats` 表名因不在表内而不受影响。

- [x] 后端码 `volte_*` → `cellular_ims_*`（`errors.rs` 等 10 个 Rust 文件）
- [x] 前端码表重新生成、formatter 与单测同步
- [x] 改名后仍无子串关系（改名脚本自检 + 三处守卫）
- [x] `test_ims_fallback_boundary.py` 同步
- [x] `format!` 前缀一并改名：`profile_not_lte_ready`、`profile_not_found_in_source`、
      `baseband_wedged`、`modem_missing_wait`、`options_ping_timeout`、
      `register_refresh_retry`、`register_refresh_failed`
- [x] `connectivity/core/register.rs` 的 2 处跨层字面量同步
- [x] 新增 Rust 自检 `codes_use_the_cellular_ims_prefix`，禁止 `volte_` 前缀回流
- 注：引用第三方历史码名的注释（beta2 / 1.7）保留原拼写，那是史实而非本项目的码

### 步骤 5 — JSON 字段与 serde 别名

`volte_ims` 的持久化 transport 值由步骤 7 带迁移处理；本步只动 serde 字段名。

- [x] `volte_profiles` / `volte_ims`（effective profile 字段）/ `volte_ready` /
      运行态 `volte` 改为写出 `cellular_ims_profiles` / `cellular_ims` /
      `cellular_ims_ready` / `cellular_ims`，旧名保留为 alias
- [x] 6 个已有 alias 的线路配置键交换 `rename` 与 `alias`：写出新名、读入兼容旧名；
      混用新旧两种拼写仍按设计 fail closed。已确认 profile 更新走类型化接口，
      不存在“旧文档 + 新字段”原样合并后反序列化的路径
- [x] SIM 覆盖配置 `ims_volte` / `ims.volte` → `ims_cellular` / `ims.cellular_ims`；
      `AccessPathKind` 写出 `"cellular_ims"`；UT 响应 `access` 同步
- [x] `frontend/src/api/contracts.ts` 同步
- [x] 其余前端文件同步（含 Dashboard、SMS、线路详情、语音路由、e2e 测试 ID）
- [x] 环境变量 `SIMADMIN_CELLULAR_IMS_PCSCF` / `SIMADMIN_CELLULAR_IMS_CID`，
      `SIMADMIN_VOLTE_*` 保留为回退

### 步骤 6 — 收尾

- [x] `bruno-api/`：5 个请求改名并指向 `/cellular-ims/` 规范路由，
      2 个路径策略请求体改为 `cellular_ims`，删除早已 404 的 `set_volte_voice.bru`，README 同步
- [x] 更新 `docs/IMS_NAMING_MIGRATION.md`：标注第一阶段“保留旧写出名”的决定已被取代，
      更正“错误码持久化”的错误记载，新增第二阶段小节
- [x] `docs/CHANGELOG.md` 追加条目
- [x] `71513ea` 的 CI 全绿，`Publish Release` 保持 `skipped`；2026-09-24 已重新查询确认。
      本轮另补迁移测试执行过滤器，验证记录见 §7（此前仅编译该测试）。

### 步骤 7 — 数据库联动（2026-09-23 纳入范围）

用户确认 `carrier_Bundles` 即“数据库项目”，要求本阶段一并处理原先留待联动的
持久化名称。§2.2 与 §6.1 的“留待联动”因此改为本步执行。持久化值不能靠文本替换，
每项都要有迁移，且读取端兼容旧值：

- [x] 表 `volte_refresh_stats` → `cellular_ims_refresh_stats`。迁移以旧表存在为触发条件
      （此前每个版本启动时都会建旧表，旧表存在 ⇔ 可能有旧名数据），把旧行并入新表后删除旧表；
      降级运行旧版后再升级，同一迁移会再跑一次
- [x] `sms_messages` / `sms_dedup` / `app_events` 的 transport `volte_ims` → `cellular_ims`，
      `app_events` 事件类型 `volte.*` → `cellular_ims.*`；后端归一化、通知标签、
      诊断日志子系统标签与前端显示仍接受旧值
- [x] 短信 MT 标记前缀 `volte-mt:` → `cellular-ims-mt:`：去重主判据是内容指纹，
      不含前缀，不受影响；按标记做的逐条查重依赖存量 `pdu`，因此迁移同时改写历史行
- [x] 新增迁移测试 `legacy_volte_persisted_names_migrate_to_cellular_ims`
      （模拟旧版在新库上写入，再次启动后逐项核对，并验证二次启动无副作用）
- [x] `carrier_Bundles` 联动审查完成：`services.volte`、`services.vonr` 是真实 LTE/NR
      语音能力，不改成 IMS 注册开关；上游提取键保持原样。schema 的注册字段已为
      `lte_ims_status` / `nr_ims_status`。
- [x] 修正数据库项目的真实语义混用：LTE/NR readiness 不再被 `volte=false` /
      `vonr=false` 否决；SMS-only IMS 可按完整配置得到 ready，缺配置仍拒绝 ready。
      新增 Python 测试；SimMaster 增加 SMS-only profile 可解析且不宣告 MMTEL 的 Rust 回归。
      旧 sealed catalog 不就地改写，新生成 catalog 才采用新判定。

## 6. 边界

### 6.1 持久化值不得仅做文本替换

`volte_ims` 会落盘且有精确匹配消费者。步骤 7 已包含存量迁移与旧值读取兼容，
不再以“错误码不落盘”为理由忽略 transport 历史数据。

### 6.2 排除项：非错误码字面量

以下虽形如 `volte_*`，但属于其他语义，不纳入错误码改名：

- 断连原因串：`volte_ip_families_changed`、`volte_profile_selection_changed`、
  `volte_line_connection_disabled`（传入 `disconnect_live_for_line`）
- 状态串：`volte_degraded`
- serde 字段名：`volte_profiles`、`volte_ready`、`volte_ims`（步骤 5 单独处理）

### 6.3 通用边界

- ~~不做 schema 变更；`volte_enabled` 列与 `volte_refresh_stats` 表名留待数据库联动。~~
  已由步骤 7 带迁移完成（`volte_enabled` 实为线路配置 JSON 键，归步骤 5）。
- `/volte/*` 路由别名不删除。
- 不改动对外提示文案的中文表述。
- 不触碰 IMS 注册逻辑本身——本阶段是纯命名与契约变更。
- Rust 构建/测试仅在 Actions 执行；本地只做 `cargo fmt`、`git diff --check`
  和 Python 结构化测试。

## 7. 变更记录

| 步骤 | 提交 | 说明 |
|---|---|---|
| — | `36a0a2e` | 开工基线 |
| — | `8cd62e1` | 本计划文档落盘 |
| 1 | `20d5bda` | 码表补齐至 158 项、194 处调用点改引用常量、新增一致性守卫 |
| 1 | `27542f0` | 补 `handlers.rs` 的 `code` 导入（修 17 处 E0433） |
| 2–3 | `afa7112` | 前端按 token 精确匹配、生成式码表、前后端码表一致性守卫；CI 全绿 |
| 4 | `a4108f3` | 错误码 `volte_*` → `cellular_ims_*`（427 处，14 个文件）；CI 全绿 |
| 5–7 | `71513ea` | JSON/配置/持久化名称迁移；CI 全绿、发布 skipped；SIM-04 已部署并自然续期 |
| 收尾 | 本轮待提交 | 数据库迁移测试加入两个实际执行过滤器；carrier_Bundles readiness 与语音能力解耦 |

步骤 1 验收：CI `Validate Beta Refactor` / `Build-Release` 全绿，
arm64 与 amd64 均编译通过，`Publish Release` 保持 `skipped`；
本地 90 项 Python 结构化测试通过（原 83 + 新守卫 7）。

步骤 2–4 验收：本地 95 项 Python 守卫、前端 8 项单测、`tsc -b` 与
`eslint --max-warnings 0` 通过；`afa7112` 与 `a4108f3` 的三个工作流均为 success。
