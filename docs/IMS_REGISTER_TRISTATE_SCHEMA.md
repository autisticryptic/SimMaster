# IMS REGISTER 三态字段 schema

> 建立日期：2026-08-29<br>
> 项目：SimAdmin<br>
> 适用：carrier catalog v7 bundle → `RegisterPolicyRecord` → `CarrierProfile` → SIP REGISTER

本文档是 REGISTER 布尔开关三态语义的唯一来源：合法取值、缺省行为和最终报文影响。相关待办见 `DEVELOPMENT_PLAN.md`。

## 1. 什么叫三态

carrier bundle 里的一个开关有三种状态，不是两种：

| 状态 | bundle 写法 | 投影结果 | 含义 |
|---|---|---|---|
| 显式开启 | `true`（JSON 布尔）或 `"true"`（字符串） | `Some(true)` → `true` | 运营商要求发这个头 |
| 显式关闭 | `false`、`"false"`、`"omit"` | `Some(false)` → `false` | 运营商要求**不要**发 |
| 无意见 | 字段缺失或 `null` | `None` → 走 baseline | 运营商没表态，用代码默认值 |

关键区别是**显式关闭**和**无意见**。两者最终都可能是 `false`，但来源不同：显式关闭必须压过代码默认值，无意见则让默认值生效。旧实现把 `"omit"` 当成"无法解析"而返回 `None`，于是默认值为 `true` 的字段会被重新打开——这是本轮修复的核心。

## 2. 取值解析规则

投影入口：`bool_at_or_omit()`，位置 `backend/src/connectivity/modems/ims/vowifi/carrier_catalog_v7.rs`。

- JSON 布尔 `true` / `false`：按字面值。
- 字符串 `"true"`：`true`。
- 字符串 `"false"`、`"omit"`：`false`。大小写不敏感，前后空白会被 trim。
- 字段缺失或 `null`：`None`，baseline 生效。
- **其他任何值都是错误**，返回 `carrier_catalog_register_bool_invalid:<pointer>:<value>`，整行 profile 被拒绝。

最后一条是刻意的。`"no"`、`"yes"`、`"disabled"`、`1`、`0` 都不是合法写法。如果这些值静默回落到 `None`，那么一个写着 `"no"`（显然是"不要发"）的 bundle 会打开一个默认值为 `true` 的头，而且发生在注册路径上、没有任何提示。bundle 写错值属于编写错误，必须可见。

`AccessIdentityPolicy` 类字段用 `access_identity_policy_at()`，合法值为 `omit` / `static` / `dynamic_if_known` / `required_dynamic`（连字符和下划线等价，大小写不敏感），非法值同样报错。

## 3. 字段清单

JSON pointer 相对于 `$.sip.common.register`。「baseline」列是字段缺失时的取值。

| 字段 | baseline | 最终报文影响 |
|---|---|---|
| `include_pani_initial` | LTE/EPC 接入为 `true`，Wi-Fi/ePDG 需显式开启 | 控制初始 REGISTER 是否带 `P-Access-Network-Info` |
| `include_pani_authenticated` | 同上 | 控制已认证和 refresh REGISTER 是否带 PANI |
| `include_route_header` | `false` | 控制是否带 `Route`。经 `RegisterRequestPolicy.include_route_header` 传到报文层 |
| `include_p_preferred_identity` | `true` | 控制是否带 `P-Preferred-Identity` |
| `always_add_sip_instance` | `true` | 控制 `Contact` 是否带 `+sip.instance` |
| `enable_cellular_network_info` | `false` | 控制是否带 `Cellular-Network-Info` |
| `require_sec_agree_headers` | `false` | 控制 `Require: sec-agree` |
| `proxy_require_sec_agree_headers` | `false` | 控制 `Proxy-Require: sec-agree` |
| `enable_initial_reject_fallback` | `false` | 控制初始 REGISTER 被拒后是否尝试兼容候选 |
| `include_mmtel_features` | 由 `services.volte` / `services.vowifi` 声明推导 | 控制 MMTEL feature tag 和 `Allow` |
| `include_visited_network` | 由 `visited_network_header` 是否存在推导 | 控制 `P-Visited-Network-ID` |

后两项不接受 `omit`：它们不是独立开关，而是从其他字段推导出来的。

### 3.1 `security_agreement` 与 `sec_agree_mode`

`security_agreement` 是字符串字段，不是布尔：

| bundle 值 | `sec_agree_mode` | 行为 |
|---|---|---|
| `"required"` | `required` | 初始 REGISTER 就发 Security-Client |
| `"auto"` 或缺失 | `auto` | 挑战驱动：收到 421/494 或具体 Security-Server offer 才发 |
| `"omit"` / `"disabled"` | `disabled` | 永不发 Security-Client 和 Security-Verify |

`sec_agree_mode = "disabled"` 与 `security_client_mechanisms` 的关系需要特别说明：

- 机制列表**保留在数据里**，往返序列化不丢，运营商切回 `auto`/`required` 时不必重新填。
- 但 live 层**不发** RFC 3329 offer。`vowifi/live.rs` 用 `sec_agree_mode != "disabled"` 同时门控 `security_client` 和 `security_verify` 两个头。
- 所以"列表非空"不等于"会发 offer"。判断是否发送只看 `sec_agree_mode`。

## 4. 链路完整性

从数据库到报文有四层，`omit` 必须逐层不丢：

```text
bundle config_json
  → bool_at_or_omit()            解析三态，非法值报错
  → RegisterPolicyRecord         裸 bool，omit 已解析为 false
  → CarrierProfile (&'static)    record.intern()，逐字段拷贝
  → RegisterRequestPolicy        register_variants() 从 profile 推导
  → SIP REGISTER 报文            build_register_from_profile_*()
```

`RegisterPolicyRecord` 用裸 `bool` 而不是 `Option<bool>`：三态只存在于 bundle JSON 层，投影后就是具体值。这是刻意的，但带来一个风险——

**序列化风险。** `include_pani_initial`、`include_pani_authenticated`、`include_p_preferred_identity`、`always_add_sip_instance` 四个字段带 `#[serde(default = "default_true")]`。任何丢字段的中间层（导出导入、部分 patch、手改数据库行）都会让 `false` 变回 `true`，也就是把运营商的"不要发"变成"发"。

回归测试：`connectivity::modems::ims::vowifi::profile_record::tests::omitted_register_switches_survive_a_json_round_trip`。

数据库加载路径是安全的：`from_database_json()` 先反序列化，再把**原始 JSON** 交给 `normalize_legacy_database_record()`，靠 presence 判断区分"缺失"和"authored false"。

**HTTP 写入路径靠显式要求补上同样的保护（2026-08-29）。** `PUT /api/vowifi/carrier-profiles` 拿不到原始 body，无法做 presence 判断，所以改为**要求**八个三态开关全部出现：缺任一个返回 400 `carrier_profile_register_switch_missing:<字段名>`，缺整个 register 段落返回 `carrier_profile_register_section_missing`。入口是 `CarrierProfileRecord::from_api_value()`，字段清单在 `REQUIRED_REGISTER_SWITCHES`。

PUT 是整体替换，所以"必须说清每个开关"本来就是这个动词的含义。前端不受影响：`contracts.ts` 里这八个字段都是非可选 `boolean`。

回归测试：`the_api_parser_refuses_a_body_missing_register_switches`、`a_partial_put_body_is_refused_by_the_live_endpoint`。

配置导入导出不受影响：`custom_carrier_profiles` 表不在 `CONFIG_TABLES` 导出范围内，二进制 restore 又是整表 `SELECT *` 原样复制。

## 5. 回归测试对应关系

| 覆盖层 | 测试 |
|---|---|
| bundle → record，`omit` 生效 | `carrier_catalog::v7::tests::explicit_omit_in_database_bundle_disables_optional_register_headers` |
| bundle 非法值被拒 | `carrier_catalog::v7::tests::wrongly_typed_register_switch_is_rejected_instead_of_defaulting` |
| bundle 合法写法全部保持可用 | `carrier_catalog::v7::tests::legal_register_switch_spellings_are_all_still_accepted` |
| record JSON 往返不翻转 | `profile_record::tests::omitted_register_switches_survive_a_json_round_trip` |
| profile → policy 不重新开启 | `volte::live::tests::register_fallback_never_reenables_explicitly_disabled_profile_capabilities` |
| record → 最终 REGISTER 报文 | `volte::live::tests::omitted_register_switches_are_absent_from_the_built_request` |

最后一项是端到端断言：从 record 经 `intern()` 和 `register_variants()` 生成真实 REGISTER 字节，在 Initial/Authenticated/Refresh 三个阶段、对候选阶梯里每个 variant 断言 `P-Access-Network-Info`、`Cellular-Network-Info`、`P-Preferred-Identity`、`Route`、`Security-Client`、`Security-Verify` 均不存在，`Require`/`Proxy-Require` 不含 `sec-agree`，`Contact` 不含 `+sip.instance`。

测试里 P-CSCF 地址是刻意填上的：`Route` 只由 profile 开关压制，不填 P-CSCF 会让断言空转。
