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
所以部署侧提供三个**显式且可变**的插槽，取值为 `$APP_VERSION` 占位符：

```jsonc
"app_version": "8.9.8.0",                                   // 静态/存储值（与 file 互斥）
"app_version_file": "/opt/simadmin/jegotrip-app-version.txt", // 每次请求重读
"app_version_web_url": "https://www.jegotrip.cn/m/mobileIndex.html", // 官方页面自动发现，30分钟缓存
"extra_query":   [ { "name": "version",   "value": "$APP_VERSION" } ],
"extra_headers": [ { "name": "version",   "value": "$APP_VERSION" } ],
"extra_body":    [ { "name": "app_version","value": "$APP_VERSION" } ]
```

解析顺序：`app_version_file` → `app_version_web_url`（30 分钟缓存，失败保留旧值）→ `app_version`。

- **不需要重新编译/重启**：改文件（`echo 8.9.9.0 > /opt/simadmin/jegotrip-app-version.txt`）
  或改存储配置（`PUT /api/jegotrip/config`）后，下一次登录立刻生效；
- 网页发现受限：仅 `https://*.jegotrip.cn/`、8 秒超时、不跟随重定向、响应≤512KB，
  只提取页面中最大的 `8.x.y[.z]` 字面量（例：`8.9.10`）；页面改版时**不猜值**，退回到旧值；
- 版本值与路径都受校验：版本仅数字与点且≤24字符，文件≤64字节；文件路径必须绝对、无 `.`/`..`；
  `app_version` 与 `app_version_file` 同时提供视为配置错误；
- 占位符解析不出来时该字段**不发送**，不会发出空版本号或默认值。

`extra_query`/`extra_headers`/`extra_body` 仍受之前的限制：每个≤16项，name 仅字母数字/`-`/`_`，
value ≤256 且不含控制字符；`n_token`/`lang`/`timestamp`/`sign`/`token` 不可被覆盖。

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
