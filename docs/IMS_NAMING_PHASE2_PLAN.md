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
**该结论不成立**。全库仅四个存错误文本的列，均不属于 IMS 注册：

| 列 | 所属表 | 实际内容 |
|---|---|---|
| `last_error` | `vowifi_soak_runs` | VoWiFi 长稳测试 |
| `last_error` | `notification_queue` | 通知发送失败原因（如 HTTP 状态） |
| `failure_code` | 通话记录 | 通话失败码，独立词表 |
| `last_failure_reason` | `volte_refresh_stats` | 仅写 `vowifi_refresh_rebuild_pending` |

`volte_refresh_stats` 表名含 volte，但只存 `refresh_count` / `last_refresh_at` /
`updated_at`。`CellularImsStatus.last_error` 是内存态，经 API 暴露，不落盘。
通知正文与系统事件均不携带 IMS 错误码。

**结论：错误码改名不存在历史数据兼容问题，唯一消费者是前端。**

### 2.2 数据库列改名不在本阶段范围

`volte_enabled` 列与 `volte_refresh_stats` 表名保留原样，等与数据库项目联动后
单独处理。本阶段不做任何 schema 变更。

### 2.3 RustRover 不适用于本次重构

两点原因：一是 RustRover MCP 当前无法连接（`Connection closed`）；二是即使连通，
这些码是**字符串字面量**而非符号，IDE 的 rename refactoring 不处理字面量内容。
可用手段只有全局文本替换，ripgrep 已足够。真正的风险在前端子串匹配（见 4.2），
需要靠守卫测试而非 IDE 解决。

## 3. 精确清点

### 3.1 错误码

- 168 个不同的 `volte_*` 错误码；含配置键共 170 个不同字面量；总计 305 处出现。
- 后端分布：

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

### 3.4 其他消费者

- `bruno-api/`：9 个文件引用 volte（含 `test_volte_status.bru`、`get_volte_line.bru` 等）。
- `.github/scripts/test_ims_fallback_boundary.py`：2 处。

## 4. 风险

### 4.1 HTTP 路由与 JSON 字段

`/cellular-ims/*` 规范路由已存在，`/volte/*` 作为别名**长期保留不删除**，
已有别名回归测试 `cellular_ims_route_aliases_keep_auth_and_response_contract`。
JSON 字段改名必须前后端同步发布，否则页面读不到字段而静默显示空值。

### 4.2 前端子串匹配错位（本阶段最高风险）

前端用 `includes()` 和正则做**子串**匹配，不是精确比较。存在 5 组互为子串的码：

- `volte_ip_families` ⊂ `volte_ip_families_auto` / `_changed` / `_duplicate` / `_empty`
- `volte_profile_selection` ⊂ `volte_profile_selection_changed`

同时前端**有意**依赖前缀族匹配，改名必须保持同族共同前缀：

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

- [ ] 在 `cellular_ims/errors.rs` 建立集中的码常量（现为散落字面量）
- [ ] `live.rs` / `channel.rs` / `pcscf.rs` / `sip.rs` / `bearer.rs` 等改引用常量
- [ ] 保留字面量值不变，本步**不改名**，只让编译器接管引用正确性
- [ ] 导出一份全量码清单供守卫读取

### 步骤 2 — 前端改为精确查表

- [ ] `cellularImsErrorFormat.ts` 从 `includes()` 改为精确查表
- [ ] 前缀族（`digest_` / `ipsec_` / `bearer_netdev_` / `at_`）显式列举成员，不再靠前缀
- [ ] 建立 `旧码 → 新码` 映射表
- [ ] 保持对外提示文案不变

### 步骤 3 — 建立一致性守卫

- [ ] 新增 Python 守卫：断言后端码集合与前端映射表键集合**完全一致**
- [ ] 断言无任一码是另一码的子串（防止错位回归）
- [ ] 接入 `beta-validation.yml` 与 `build-release.yml` 的测试过滤器
- [ ] 确认守卫在故意引入不一致时会失败（负向验证）

### 步骤 4 — 统一改名

- [ ] 后端码 `volte_*` → `cellular_ims_*`
- [ ] 前端映射表同步
- [ ] 20 个 Rust 测试文件的断言同步
- [ ] `test_ims_fallback_boundary.py` 同步

### 步骤 5 — JSON 字段与 serde 别名

- [ ] 为 `volte_profiles` / `volte_ims` / `volte_ready` 补 `cellular_ims_*` alias
- [ ] 6 个已有 alias 的键交换 `rename` 与 `alias`，改为写出新名、读入兼容旧名
- [ ] `frontend/src/api/contracts.ts` 同步
- [ ] 其余 8 个前端文件同步

### 步骤 6 — 收尾

- [ ] `bruno-api/` 9 个文件更新（文件名与请求体）
- [ ] 更新 `docs/IMS_NAMING_MIGRATION.md`，修正"stored in the database"的错误记载
- [ ] `docs/CHANGELOG.md` 追加条目
- [ ] CI 全绿，`Publish Release` 保持 `skipped`

## 6. 边界

- 不做 schema 变更；`volte_enabled` 列与 `volte_refresh_stats` 表名留待数据库联动。
- `/volte/*` 路由别名不删除。
- 不改动对外提示文案的中文表述。
- 不触碰 IMS 注册逻辑本身——本阶段是纯命名与契约变更。
- Rust 构建/测试仅在 Actions 执行；本地只做 `cargo fmt`、`git diff --check`
  和 Python 结构化测试。

## 7. 变更记录

| 步骤 | 提交 | 说明 |
|---|---|---|
| — | `36a0a2e` | 开工基线 |
