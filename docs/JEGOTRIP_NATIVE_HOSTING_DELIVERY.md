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
"app_version_file": "/opt/simadmin/jegotrip-app-version.txt", // 每次请求重读
"app_version_web_url": "https://www.jegotrip.cn/m/mobileIndex.html", // 官方页面
"app_version_ios_app_id": "1037589370",                      // iOS App Store 数字 id
"app_version_store": true,                                    // 允许写入共享状态表
"extra_query":   [ { "name": "version",   "value": "$APP_VERSION" } ],
"extra_headers": [ { "name": "version",   "value": "$APP_VERSION" } ],
"extra_body":    [ { "name": "app_version","value": "$APP_VERSION" } ]
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
{ "sso_token": "<登录后拿到的 App token>", "equipment_type": 1, "device_id": "<设备标识>" }
```

对应的服务端路径（APK 内端点目录 `JegoTripApi.<clinit>`）是
`service/voip/v1/verificationTrustfulDevice`；调用方读回 `body.code` 与 `body.authentication_code`。

SimAdmin 的实现方式：

- 部署配置新增**可选**端点（三者都缺省时行为与以前完全一致）：
  ```jsonc
  "endpoints": {
    "device_enroll": "device/sec/v1/enroll",                          // 安全设备注册，body 只有 {"deviceId":…}
    "security_verify": "service/voip/v1/securityVerify",               // 设备安全校验，body 只有 {"deviceId":…}
    "verification_trustful_device": "service/voip/v1/verificationTrustfulDevice"
  }
  ```
- 流程在 `csb/token` 之后、业务开关之前依次调用它们，并把服务端返回原样写成有界日志：
  `app_login_device_trust step=enroll|security_verify authentication_code=<…>`、
  `app_login_trust_decision authentication_code=<…>`；
- 这些返回值**只是报告**，不是授权凭据：程序不会把它们当成“已授信”去跳过任何校验，
  业务是否真正开启仍由 `setVoip` + 读回确认决定；
- `setVoip` 返回 `506` 时，任务以专用错误码 `app_login_device_untrusted` 结束（前端有对应中文提示），
  不会退化成“通用拒绝”或伪造成功。

**边界**：原 App 在 `authentication_code != 1` 时还有 `getToken()` 与运营商侧能力参与（静态确认存在），
这部分**未实现**，也不会用假值绕过。410 实测：`verificationTrustfulDevice` 返回
`authentication_code=2`（表示该 `device_id` 未被本账号授信），随后 `setVoip` 仍为 `506`；
`device/sec/v1/enroll` 与 `securityVerify` 的 `{deviceId}` 调用已接入并可观测，但它们能否把设备
变成授信设备取决于服务端策略，**未验证前不声称可用**。
`enrollId` 只与登录侧一键登录有关（`AccoutLogic.oneKeyLogin/ssoLogin/thirdOneKeyLogin`），
不是 `setVoip` 的授信凭证。静态证据见 `.local/jegotrip/device-trust-20261103/FINDINGS.md`。

### 2.2.6 已实测的授信结论（2026-11-03，410 + 真实服务）

按原 App 顺序完整复现后，实测链路是：

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
`receiveToken resultCode=` 就是它的回调。这一步需要**账号绑定号码的 SIM 在现场**（运营商网络侧校验），
纯软件无法复制；本客户端不会伪造 token 或把 `authentication_code=2` 当成已授信。

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

**结论**：代码侧已把原 App 的业务开通/接管链路补齐；剩下的唯一外部条件是“首次开启托管必须在
手机 App 完成”（服务端安全设备认证要求绑定号码的 SIM）。开启后本机用 `attach_hosting` 即可接管：
取 phone token/SIP 密码 → 建本地账号 → 启动原生接收器。

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

- **410 设备部署**：需要先核对 pinned host key、机器身份、架构、路径与当前状态，再部署
  同 SHA 的 ARM64 后端/前端与私有配置；本文件不代表已部署。
- **真实账号登录与托管验收**：需要用户自有有效凭据与自有号码，属于单独授权范围。
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
