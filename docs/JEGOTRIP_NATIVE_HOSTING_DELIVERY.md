# 无忧行原生托管：交付状态与运行说明

> 本文件只描述**当前受支持的原生子集**及其验证证据。它区分「已在 Actions + 官方制品核验的软件行为」与「仍需真实设备/账号/服务验收的部分」，不把前者当作后者。
> 私有部署值、账号 token、SipPwd、证书私钥、内置密钥一律不在此文件、不在 Git 中。

## 1. 已验证的软件状态

| 主题 | 状态 | 验证方式 |
| --- | --- | --- |
| 原生 Router/SARC/Common actor（默认 ARC 运行时选择） | 已验证 | 两个 GitHub Actions 工作流成功 + 官方 artifact digest/ZIP CRC/精确测试名 |
| `getAppConfigs` bootstrap handoff（独立 incarnation，源 actor 先 Join 释放再拨目的地） | 已验证 | 同上（真实双连接 Router/Common 对端） |
| 存储配置自动装配（`router_common` + `bootstrap_via_router`） | 已验证 | 同上 + App 登录模板 bind 测试 |
| 真实 `current_user_uri` 派生（= 匹配 loginSession 的规范化身份） | 已验证 | 账号级端到端测试断言线上 `current_user_uri` |
| 默认 `#SipGateway` 托管对象 | 已验证 | 同一测试断言嵌套 `updateStatus` 请求的 `id` |
| `mainAccountServer`/`mainAccount` 捕获与拒绝策略 | 已验证 | `rpc_login`/`arc_session` 回归 |
| `AccountAlive` 通道分类（Absent/Disabled/Unsupported，disable 哨兵字面量） | 已验证 | `runtime_provision` 回归 |
| `Sip.MessageGw.Oid` 原 scope 内缓存（不透明、有界、零化） | 已验证 | `rpc_incoming` 回归 |
| 账号级托管闭环：登录→自动注册→`Sip.Request` 短信落库→原 DIRECT ACK→注销→logout→stopped | 已验证 | `runtime_backend::common_tests` 账号级端到端测试 |

最近一次包含上述全部内容且两工作流 + 官方制品审计通过的源码提交见
`.local/jegotrip/native-integration-20261031/ci-*-status.json` 与
`.local/jegotrip/verified-native-*.json`（本地私有证据目录）。每次提交的 run id、
artifact SHA-256、精确测试数与失败证据都保留在 `.local/jegotrip/`。

## 2.1 App HTTPS 的显式证书固定（`app2.cer`）

原 App 并未依赖公共 CA：`assets/app2.cer` 就是服务端叶子证书
（`CN=*.jegotrip.com.cn`，issuer `CN=Jegotrip`，序列号 `ba770f6aa86973eb`），
而服务端只下发这一张证书、不下发中间/根证书。因此部署配置可使用新可选项
`http.pinned_server_certificate_der_base64`：

- 与 `http.root_der_base64` **互斥**；提供后不再使用任何系统根证书；
- 只接受与该 DER **逐字节相同**的服务端证书，否则握手失败（`app_login_transport_failed`）；
- 该叶子是 **X.509 v1**（无扩展字段），现代 webpki 无法解析，因此还需同时提供
  `http.pinned_server_public_key_der_base64`（该证书的 PKCS#1 RSAPublicKey DER）；
  握手签名用 ring 按 RSA-PSS/PKCS#1 方案**真实验签**，不是“接受任意证书”；
- 缺少其中任意一项、两项同时给、或给根证书与 pin 同时给，均被配置校验拒绝；
- 这是安装侧显式给定的一张证书，不是默认值、不是 CA 推断。

设备上该证书需随部署私有配置提供；校验失败会在登录 attempt 上以
`app_login_transport_failed` 结束，不会伪造成功。

## 2.2 App 请求的版本门槛与**动态**版本值

真实服务在 `setVoip`（开启托管）会校验客户端 APP 版本：实测返回 `524`
（`msg` 提示升级到 8.9.8 及以上；当前 APK 字节为 `8.9.7.0`）。版本不在原 App 的 URL/
`setVoip` 请求体里（已核对 DEX 的 `HttpRequestUitls.main/withToken` 与 `setVoipCsState`），
所以部署侧提供**显式且可变**的插槽，取值为 `$APP_VERSION` 占位符。

### 2.2.1 版本值存在哪里（共享状态表）

版本号不是编译期常量，也不靠猜：它保存在数据库的共享状态表里（与其它共用信息同一张
key/value 表，`jegotrip_shared_state`，`name='app_version'`），一行记录 `value` / `source` /
`observed_at`：

- `source` 记录来源（`manual` 手工覆盖、`ios` App Store、`page` 官方页面、`store` 读自该表），
  便于运维判断这个值是谁写的；
- 首次运行时表为空 → 先按配置的来源拉取一次 → 写入该表 → **再**开始登录流程，因此第一次
  请求就带上真实版本；
- 数据库里已存在未过期（6 小时）的值时直接用，**不依赖网络**也能启动；过期后会尝试刷新，
  刷新失败仍沿用旧值（旧值好过没有值）。

### 2.2.2 来源与解析顺序

```jsonc
"app_version": "8.9.8.0",                                    // 静态/兜底值
"app_version_file": null, // 若改为绝对路径，须同时将 app_version 置为 null
"app_version_web_url": "https://www.jegotrip.cn/m/mobileIndex.html", // 官方页面
"app_version_ios_app_id": "1037589370",                      // iOS App Store 数字 id
"app_version_store": true,                                    // 允许写入共享状态表
// 实测结论：版本只通过请求头生效，query/body 都是错的（body 额外字段会让 csb/token 直接 400）
"extra_headers": [ { "name": "app-version", "value": "$APP_VERSION" } ],
"extra_query":   [],
"extra_body":    []
```

解析顺序：`app_version_file`（每次重读）→ 共享状态表 → iOS App Store → 官方页面 → `app_version`。

- **iOS 来源**：由 SimAdmin 自己拼 `https://itunes.apple.com/lookup?id=<数字id>&country=cn`，
  配置项只接受纯数字 id，不能填任意 URL；只取 `results[0].version` 且必须是 `8.x[.y[.z]]`
  的字面量，其它值（9.x、乱码、空结果）一律视为“没有值”；
- **官方页面**：仅 `https://*.jegotrip.cn/`、8 秒超时、不跟重定向、响应≤512KB，只提取页面中
  最大的 `8.x.y[.z]` 字面量；
- **不需要重新编译/重启**：改文件或 `PUT /api/jegotrip/config` 后，下一次登录立刻生效；
- 版本值与路径都受校验：版本仅数字与点且≤24字符，文件≤64字节；文件路径必须绝对、无 `.`/`..`；
  `app_version` 与 `app_version_file` 同时提供视为配置错误；
- 占位符解析不出来时该字段**不发送**，不会发出空版本号或默认值。

### 2.2.3 服务端说“版本过低”时怎么办

登录链路任一调用返回业务码 `524` 时：重新拉取一次版本（App Store → 官方页面）→ 写入共享
状态表 → 用新值**重放整个登录流程一次**（body/query/header 全部重新构造，签名/时间戳/nonce
也都是新的）→ 若仍为 `524` 则如实失败，不再重试。只重试一次，且只在拿到**不同**版本号时才
重放，避免对同一份配置反复打服务端。

### 2.2.4 运维接口（不导出任何密钥）

```
GET  /api/jegotrip/app-version          # 当前存储值 + 来源 + 年龄 + 各来源是否已配置
PUT  /api/jegotrip/app-version          # {"version":"8.9.8.0"} 手工写入（source=manual）
POST /api/jegotrip/app-version/refresh  # 立即强制拉取一次并落库
```

返回体只有 `version` / `source` / `observed_at` / `stored_age_seconds` / `store_ttl_seconds`
与四个 `configured_*` 布尔值，不包含任何部署凭据。

`extra_query`/`extra_headers`/`extra_body` 仍受之前的限制：每个≤16项，name 仅字母数字/`-`/`_`，
value ≤256 且不含控制字符；`n_token`/`lang`/`timestamp`/`sign`/`token` 不可被覆盖。

## 2.2.5 可信设备校验（业务码 506）与两种托管模式

过了版本门槛后，`setVoip` 仍可能返回 `506 未授信设备,开启失败!`。这不是版本问题：原 App 在
调用业务开关之前会先向服务端查询“本机是否已授信”，即 `CallLogic.querySafeDevice`，请求体恰好三个
字段：

```json
{ "sso_token": "<原 App 回调提供的认证 token，来源仍须核对>", "equipment_type": 1, "device_id": "<设备标识>" }
```

对应的服务端路径（APK 内端点目录 `JegoTripApi.<clinit>`）是
`service/voip/v1/verificationTrustfulDevice`；调用方读回 `body.code` 与 `body.authentication_code`。

SimAdmin 的实现方式：

- 部署配置新增**可选**端点（三者都缺省时行为与以前完全一致）：
  ```jsonc
  "endpoints": {
    "device_enroll": "api/device/sec/v1/enroll", // 匿名调用，body 只有 {"deviceId":…}
    "security_verify": "api/service/voip/v1/securityVerify", // body 只有 {"deviceId":…}
    "verification_trustful_device": "api/service/voip/v1/verificationTrustfulDevice"
  }
  ```
- 流程在 `csb/token` 之后、业务开关之前依次调用它们，并把服务端返回原样写成有界日志：
  `app_login_device_trust step=enroll|security_verify authentication_code=<…>`、
  `app_login_trust_decision authentication_code=<…>`；
- 这些返回值**只是报告**，不是授权凭据：程序不会把它们当成“已授信”去跳过任何校验，
  业务是否真正开启仍由 `setVoip` + 读回确认决定；
- `setVoip` 返回 `506` 时，任务以专用错误码 `app_login_device_untrusted` 结束（前端有对应中文提示），
  不会退化成“通用拒绝”或伪造成功。

**边界**：HTTP 探针的存在不等于原 App 认证流程已完成。`querySafeDevice` 的 `sso_token` 从原 App
`getToken` 回调传入，不能仅凭字段名认定为 App 登录会话。当前客户端以 App 会话做诊断性查询，
尚未提供运营商号码认证凭证；服务端实测 `authentication_code=2`、`setVoip=506`。
`enrollId` 已取得并用于登录，但不能把它当成设备授信凭证。静态证据见
`.local/jegotrip/device-trust-20261103/`；任何合法替代验证路径也必须单独核验，不能伪造 token/验证通过。

### 2.2.6 已实测的授信结论（2026-11-03，410 + 真实服务）

当前客户端已实际尝试的链路是（不等同于原 App 所有认证分支均已复刻）：

```
ensure_app_version → device/sec/v1/enroll（匿名 n_token）→ 拿到 enrollId(32位)+expireTime
→ 登录 body 携带 enrollId → csb/token → securityVerify → verificationTrustfulDevice
→ getCallStatus → setVoip
```

日志证据（有界，不含凭据）：

```
app_login_device_trust_shape step=enroll enrollId=str:32 expireTime=str:19
app_login_device_enroll id_len=32
app_login_device_trust_shape step=security_verify authentication_code=num:2
app_login_trust_decision authentication_code=Some(2)
app_login_reject_fields endpoint=api/service/voip/v1/setVoip code=506 msg="未授信设备,开启失败!"
```

即：**`enrollId` 已拿到并已随登录上报，但本设备仍未通过安全设备认证（`authentication_code=2`）**。
APK 里承担这一步的是中国移动一键认证 SDK（`com.cmic.sso.sdk.auth.AuthnHelper`，封装在
`com.cmi.mobile.auth.MobileAuthUtil`），`TrusteeShipVerActivitiy.getToken()` 的
`receiveToken resultCode=` 是相关回调。认证 token、SIM/运营商通道和设备标识是不同材料；
当前证据仅证明本客户端尚未通过授信，不能由字符串扫描推出所有账号/版本都只能用同一种验证方法。
本客户端不会伪造 token，或把 `authentication_code=2` 当成已授信。

因此 SimAdmin 提供**两种托管模式**（`intent`）：

| intent | 行为 | 适用场景 |
| --- | --- | --- |
| `enable_hosting`（默认） | 完整序列，必要时调用 `setVoip` 开启托管；若服务端以 506 拒绝，任务以 `app_login_device_untrusted` 结束 | 设备已授信（或服务端不要求）的环境 |
| `attach_hosting` | **绝不写业务开关**：先读 `getCallStatus`，只有服务端确认 `cs_forward=2`（已开启）才继续建账号并启动接收器；否则以 `app_login_business_not_enabled` 结束 | 已在手机 App 开启托管的账号，本机只接管接收 |

实测（真实服务，账号当前托管未开启）：

```
POST /api/jegotrip/login/attempts  {intent:"attach_hosting"} → 202
phase: queued → checking_business → failed  error=app_login_business_not_enabled
business_state=not_requested   （证明没有任何写业务开关的请求）
```

**结论**：已实现并测试 HTTP 登录、授信查询和接管入口，但尚未通过真实服务的设备授信，
也尚未完成真实账号的完整原生接收验收。手机 App 的合法开通可作为后续接管的前置条件；
不能据此宣称“只差开通、后续必然成功”。开通后仍须逐项验证原生会话、短信、通话/媒体和停止清理。

### 2.2.7 已验收的真机结果（2026-11-03，`4ad682b9`）

| 能力 | 真实服务结果 |
| --- | --- |
| App 登录 + 绑定号码 + 账号记录（`intent=authenticate`） | **通过**：`completed=true`、`masked_number=***7634`、生成本地账号 `f70bc6cd…` |
| 版本门槛（`app-version` 头） | 通过（`524` 消失） |
| 设备注册 `device/sec/v1/enroll` | 通过：返回 `enrollId`(32) + `expireTime`(19)，已随登录上报 |
| 安全设备校验 `securityVerify` / `verificationTrustfulDevice` | 已调用，服务端回答 `authentication_code=2` |
| 开启托管 `setVoip` | 仍 `506 未授信设备`（需手机侧一键认证） |
| 接管 `intent=attach_hosting` | 正确拒绝：`app_login_business_not_enabled`，`business_state=not_requested`（零写入） |

接管未开启业务的拒绝路径已通过实测；已开启业务的真实接收路径仍待验收，不能视为已经可用。

### 2.2.8 APK 能力 → SimAdmin 对照表（托管相关）

| 能力 | APK 端点 / 组件 | SimAdmin 状态 |
| --- | --- | --- |
| App 登录 | `userCenter/v3/signIn` | ✅ 已实现，真实服务通过 |
| 绑定号码 | `service/user/v1/getUser` | ✅ 已实现 |
| 电话 token | `service/voip/v1/csb/token` | ✅ 已实现 |
| SIP 凭据 | `service/voip/v1/openVoipStatus` | ✅ 已实现（内存内 AES/MD5 解密） |
| 托管状态 | `service/voip/v1/getCallStatus` | ✅ 已实现 |
| 开启托管（不自动关闭） | `service/voip/v1/setVoip` | 已实现请求/读回确认；真实服务仍 `506`，关闭业务未实现 |
| 安全设备注册 | `device/sec/v1/enroll` | 已实现（匿名调用，`enrollId` 随登录上报） |
| 安全设备校验 | `service/voip/v1/securityVerify` | 已实现 HTTP 查询，实测返回未授信 |
| 可信设备查询 | `service/voip/v1/verificationTrustfulDevice` | 已实现诊断查询，原 App 认证 token 来源/授信尚未闭合 |
| 一键认证（号码/SIM） | `com.cmic.sso.sdk.auth.AuthnHelper` / `MobileAuthUtil` | 未实现；不能由设备标识替代真实认证材料 |
| 在线状态上报 | `api/service/voip/v1/setJegoMStatus` | ⛔ 未实现（UI 层在线上报，语义未确认） |
| 号码服务开关 | `service/voip/v1/phoneService` | ⛔ 未实现（语义未确认，不是托管开关） |
| App 退出 | `service/user/v1/logout` | ⛔ 未实现（本机不持久化 App 会话） |
| 托管资料 | `service/voip/v1/getProfile` | ⛔ 未实现（当前用安装侧静态运行模板） |
| 原生托管会话（ARC/SARC/Router/TLS） | `libmtc` / SDK 层 | ✅ 已实现（离线端到端测试覆盖） |

### 2.2.9 完成一次完整托管的操作步骤

1. **手机 App**（绑定号码所在手机）：登录账号 → 号码托管 → 开启（该步走中国移动一键认证，由运营商校验 SIM）。
2. **SimAdmin**：登录方式选“接管已开启的托管（先在手机 App 开启）”，提交同一账号；此时服务端 `cs_forward=2`，程序会
   取 phone token / SIP 密码 → 建本地账号 → 启动原生接收器。
3. 向托管号码发一条短信，在 SimAdmin 的短信列表确认入库（分类 `smgw_text`）。
4. 在账号上执行 `stop`，确认原生注销 → SDK `logoutSession` → 会话 stopped，且服务端托管开关保持开启。
   `stop` 不调用 App HTTP logout，也不关闭业务；这些 API 与原生会话清理是不同层。

## 2.3 设备标识：默认随机、持久复用、手工指定

这是**客户端标识管理**，不是绕过认证。一个标识不借给不同号码是本项目的隔离策略；朋友描述的
服务端全局限制仍须实测，不能把此策略当作已证明的全部服务端规则。

- **默认 `store`**：新安装不必填写 `http.device_id`。首次明确提交登录前生成大写 UUID v4 并落库；
  同一登录身份在重试、密码更改和重启后复用。旧配置已有固定值时，仅首次保留原值，避免擅自旋转；
  其他新账号仍独立生成，不共用旧值。
- **按认证号码绑定**：预登录只记服务域 + 登录身份的哈希；登录成功并取得绑定号码后，才与规范账号 key
  关联。一个号码的手机号/邮箱等别名收敛到同一标识。新别名若发现已有标识，会在任何电话 token、
  授信或业务开关请求之前，以该标识重新认证一次；号码发生不一致则失败，不转移旧所有权。
- **手工指定**：首次登录可选“手工指定自己的标识”，填写 8–64 位字母、数字、`_`、`-`。
  已有标识更换必须用下方“设备标识管理”的版本守卫操作，不能用普通登录请求默默覆盖。
- **配置兼容**：`http.device_identity_mode` 可为 `store`（默认）或 `static`。后者要求显式 `device_id`，
  仍遵守同样的唯一性/号码隔离；不能把一个固定值交给多个号码使用。
- **分层**：这里只管理 App HTTPS 的 `deviceId` / `device_id`。SDK 的显式设备参数、物理 IMEI、SIM、
  IMS/NV 均不修改，不能假定它们与这个 UUID 相同。

### 2.3.1 专用表和保护规则

| 表 | 内容 |
| --- | --- |
| `jegotrip_device_identities` | 本地行 id、当前标识、生成来源、规范账号哈希、版本、弃用标记及时间 |
| `jegotrip_device_aliases` | 登录域/登录别名的哈希 → 设备标识行（不保存明文账号、密码） |
| `jegotrip_device_identity_history` | 旧标识的保留所有权，阻止改名/删除后被另一号码复用 |

有效行≤64、别名≤256、历史值≤512；达到上限明确拒绝，不覆盖旧记录。列表及字段复制均有界，
坏的持久化行会返回存储错误而非隐藏/反射秘密。设备值仅在鉴权后的页面/API 显示，Debug/错误不回显。

同一事务完成标识预留和登录任务入库；繁忙/无效/存储失败的提交不会留下半份新标识。
登录期间禁止修改标识；接收器的任何旧运行租约/未清理收件 fence 都会阻止旋转和弃用，
不只检查最新 `voice_line_id`。改写使用 `expected_revision`；省略 `account_id` 不清空原号码绑定。
删除是弃用记录，旧值不释放给别人，也不擅自停止接收器或关闭远端业务。

### 2.3.2 鉴权 API / 中文 UI

```text
GET    /api/jegotrip/device-identities
PUT    /api/jegotrip/device-identities
DELETE /api/jegotrip/device-identities/{本地行id}?expected_revision=<版本>
```

PUT 示例（全新标识）：

```jsonc
{"id":null,"expected_revision":0,"account_id":null,"device_id":null} // 后端随机生成
{"id":null,"expected_revision":0,"account_id":null,"device_id":"OWN-DEVICE-0001"} // 指定自己的值
```

修改已有行时提供页面返回的 `id` 和 `revision`，可选绑定已有账号 id；重复/跨号码占用、过期版本、
使用中的修改分别返回明确冲突，绝不自动重试生成。返回只含标识、来源、脱敏号码及使用/版本元数据，
不含账号 key、别名 key、密码、App/SIP token 或 SDK 配置。URL 使用本地行 id，不放真实设备值。

登录页新增“自动生成/复用”和“手工指定”，下方可生成、指定、绑定、更新、弃用和刷新。
已绑定账号下次自动复用；预先生成的未分配值只有在首次登录明确选择其值时才会使用，
不会从全局空闲池随机借给另一个账号。页面关闭清空输入，不写浏览器存储。

新增回归覆盖 SQLite 重开/并发、别名归并、换号拒绝、CAS/旧值保留、旧接收器 fence、坏行有界拒绝、
真实鉴权管理路由和全部 App 设备字段一致性。两个工作流另外执行仅模拟 API 的桌面/手机 UI 测试；
截图和报告保存在官方 `jegotrip-device-ui-*` 制品。它们与 410 的真实业务验收分开记录。
相关本地收据/失败证据：`.local/jegotrip/device-identity-20261103/` 与 `verified-native-*.json`。

## 2. 受支持的配置形状（不含真实值）

存储的账号配置里，原生路由需要**同时**给出：

```jsonc
{
  "credentials": {
    "sdk": {
      // bootstrap 阶段使用的原始逻辑入口（不是最终目的地）
      "server_address": "<BootstrapEntry>:sarc -h <logical-host> -p <port>;",
      "root_der_base64": "<仅占位，native 不使用>",
      "client_config_decode_key": "<Client 属性解码上下文>",
      "reference_decode_key": "<Common 引用解码上下文>"
    }
  },
  "profile": {
    "route": {
      "router_common": {
        "reference": "Router:tls -h <router-host> -p <router-port> -ri <router-id>;",
        "anchors": [ { "router_id": <id>, "host": "<router-host>", "port": <port>,
                       "authority": "<cert-name>", "root_der_base64": "<router root>",
                       "enable_sni": false } ],
        "access": [],
        "endpoint_index": 0, "resolved_host_index": 0,
        "startup_timeout_ms": 5000, "attempt_timeout_ms": 8000, "compress_above": null
      }
    },
    "authority": "", "enable_sni": false,
    "domain_name": null, "domain_id": null, "application_id": null, "device_info_cc": null,
    // 空字符串 = 由匹配的 loginSession 身份派生 current_user_uri
    "gateway_uri": "",
    // 省略 id 时 Hosting 自动使用原默认对象 #SipGateway
    "gateway_metadata": [],
    "bootstrap_via_router": { "routing": [], "handoff_timeout_ms": 10000 }
  }
}
```

规则（实现即断言）：

- bootstrap 模式下**目的地未知**：`server_address` 只是 bootstrap 入口，最终逻辑入口与
  `DomainName/DomainId/AppId/CC` 只能来自匹配的 `getAppConfigs` 回复；预填这些字段、
  预填 `id`、非零 endpoint/resolved 偏移都会被拒绝。
- 空 `gateway_uri` **仅**在该路由合法；其他路由必须显式给出 URI。
- 显式给出的托管对象 `id` 会被保留；未给出才注入 `#SipGateway`。
- Handoff 预算 ≤120 秒，且实际 deadline = min(attempt 预算, handoff)。
- 未获匹配回复前不建立目的地 TCP；bootstrap actor 先 Join 释放。

## 3. 运行步骤（本机/设备一致）

1. 写入账号配置（通过既有 `/api/non-3gpp/accounts` 或 App 账号登录主入口；私有部署模板
   可以携带上面同一份 `profile`）。
2. `POST .../start`（或 App 登录主入口）→ 提交阶段返回 `sdk_login_pending`。
3. 轮询账号视图直到 `phase == "hosted"`；此时按能力分别观察：
   - `incoming_message_ready`（需要 `incoming.receiver_kind == "Sip.Request"`）
   - `incoming_call_ready`（需要通话配置）
   - `provider_availability` 仅在对应能力就绪时变为 `available`
4. 收件：`GET .../messages`、`GET .../inbox`。短信只有在原始正文、`Sip.Message.From`、
   可选 `Sip.Message.Time` 全部符合受支持文本子集时才分类为 `smgw_text`；其他保持
   `unclassified` 且不写短信表。原 DIRECT/batch 票据在本地提交**之后**才 ACK。
5. `stop`：先 `updateStatus(SipFlag=3)` 注销，再 `logoutSession`，全部 Join 完成才标记
   `stopped` 并释放运行记录。未 ACK 的注销请求可能被原始批次重传，属正常行为。

排障要点：`last_error` 与 `phase` 是权威；不要把 socket 关闭、HTTP 200 或 SDK 回调单独
当作托管成功。

## 4. 明确未完成 / 需单独授权的部分

- **410 设备部署**：使用用户授权的既有安全通道，核对 pinned host key、机器身份、架构及同 SHA 官方制品；
  实际安装提交/运行哈希以 `.local/jegotrip/deploy-410-*/` 的守卫与复验收据为准，部署成功不等于业务可用。
- **真实账号与托管验收**：App 登录/绑定号码已有实测；服务端授信/业务开启、原生接收短信/通话/媒体及
  停止清理仍须分别验证。手机号认证材料不能由 UUID、enrollId、HTTP 200 或本地布尔值代替。
- **真实服务语义未闭合项**：服务端是否强制要求 `registerAlive.AccountAlive.Account`
  （当前只做分类，不乐观注册）、`Mtc_UeSetStatus` 的状态取值、以及任何服务端并发/设备
  绑定规则。没有实测证据前不实现猜测行为。
- **不支持**：SRTP/ICE/P2P、浏览器 WebRTC、外呼/发短信真实发送、Locator 完整解析、
  NetStream 全部可靠性语义、跨运行 exactly-once。

## 5. 失败保留原则

所有门禁失败都保留原始失败 ZIP/日志（`.local/jegotrip/*-failure-evidence.json`），只修复
根因，不删除测试、不放宽断言。历史失败示例：预先提供 bootstrap 目的地被静默忽略（改为
admission 错误）、测试 Reader 少读一个字段、`Sip.MessageGw.Oid` 被错误字符集过滤、批次
重传导致“单请求”断言过严、跨 `break` 借用报文（E0597）。
