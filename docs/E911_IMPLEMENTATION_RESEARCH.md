# SimAdmin E911、TS.43 与 VoWiFi 紧急地址实现调研

> 文档状态：基于 2026-08-10 的公开资料与当前 SimAdmin 工作树进行静态调研。
>
> 本文只讨论架构、协议和后续开发，不表示 SimAdmin 已经完成 E911 地址登记或紧急呼叫认证。调研过程没有访问任何用户的运营商账户、提交真实地址或拨打 911。

## 1. 结论

E911 不是把地址写进 SIM，也通常不是把完整地址放进 SIP REGISTER。北美 VoWiFi 的常见实现是以下三部分共同完成：

1. 设备上的 entitlement 客户端使用 SIM 身份访问运营商服务器。
2. 运营商 entitlement server 返回当前 VoWiFi、条款和紧急地址状态；需要用户操作时，再返回运营商 websheet。
3. 用户在设备承载的运营商页面中登记地址，运营商验证并保存地址；设备随后重新查询 entitlement 状态。

因此，“运营商网页设置”和“手机端设置”并不矛盾。用户通常从手机设置进入，但表单和最终数据属于运营商：

```text
手机/设备设置入口
    → SIM EAP-AKA 鉴权
    → 运营商 entitlement server
    → 运营商 websheet
    → 运营商 E911/provisioning 数据库
```

Apple 的公开文档只描述用户操作，没有公开 iOS 内部协议；Android 则公开实现了 GSMA TS.43，并且提供完整 AOSP 源码。SimAdmin 最适合优先实现兼容 TS.43 的 entitlement client，再为经过验证的非标准运营商增加 adapter，而不是抓取普通账户网页或猜测私有接口。

## 2. 必须区分的三个概念

### 2.1 紧急地址登记

用户向运营商登记一个 civic/dispatchable address。它通常作为 Wi-Fi Calling 无法取得可靠动态位置时的注册位置或后备位置。

### 2.2 VoWiFi entitlement

运营商判断当前订阅、设备和业务是否允许使用 VoWiFi。地址、条款和 provisioning 都可能是 entitlement 的组成条件。

Android AOSP 将 VoWiFi 可用性拆成：

- `EntitlementStatus`
- `ProvStatus`
- `TcStatus`
- `AddrStatus`

这四项满足运营商策略后，客户端才把 VoWiFi 视为 entitled。

### 2.3 紧急呼叫本身

911 呼叫的识别、IMS emergency registration、路由、位置传递、callback 和 CS fallback 是另一条完整链路。地址登记成功不自动证明紧急呼叫链路已经实现或通过认证。

SimAdmin 必须分别报告：

```text
emergency_address_provisioning
emergency_calling
```

不得使用一个 `e911_enabled=true` 同时表示两者。

## 3. GSMA TS.43 的标准角色

[GSMA TS.43 Service Entitlement Configuration](https://www.gsma.com/newsroom/gsma_resources/ts-43-service-entitlement-configuration/) 描述设备客户端与运营商 Entitlement Configuration Server 之间的业务配置交换。目前覆盖 VoWiFi、VoLTE/VoNR、SMSoIP、部分 eSIM ODSA 等业务。

这里的 entitlement 表示某项服务对当前设备和订阅的适用性、可用性与状态。它不是 IMS SIP 注册协议本身。

典型流程如下：

```text
1. 从可信 carrier config 取得 entitlement server URL
2. 设备向 server 发起 HTTPS entitlement query
3. server 返回 EAP-AKA challenge
4. 设备通过当前 SIM/UICC 计算 AKA response
5. server 验证订阅者并返回 TS.43 配置/状态
6. 如果条款或紧急地址缺失：
   - 返回 TS.43 WAP XML `ServiceFlow_URL`（旧资料/部分实现称 `ServerFlow_URL`）
   - 返回 `ServiceFlow_UserData`（旧资料/部分实现称 `ServerFlow_User_Data`）
7. 设备在受控 WebView 中 GET/POST 运营商页面
8. 用户完成条款或地址登记
9. websheet 通知客户端流程完成
10. 客户端重新查询 entitlement
11. 运营商确认 AddrStatus/ProvStatus/EntitlementStatus
```

需要注意：地址页面 URL 不一定是可直接公开访问的普通网页。它可能依赖前一次 EAP-AKA 会话生成的 token、cookie 或 `ServerFlow_User_Data`。

## 4. Android/AOSP 的公开实现

### 4.1 官方架构说明

[Android IMS service entitlement](https://source.android.com/docs/core/connect/ims-service-entitlement) 明确说明：

- Android 12 起支持 GSMA TS.43。
- entitlement client 通过运营商服务器查询 IMS 服务状态。
- 身份验证使用 EAP-AKA，并通过 telephony/UICC API 完成，不要求用户手工输入运营商账户密码。
- 北美运营商可以使用该功能管理 emergency address。
- 运营商需要 web portal UI 时，Android 会显示 WebView，例如接受条款或填写紧急地址。

相关 CarrierConfig 包括：

| CarrierConfig | 用途 |
| --- | --- |
| `KEY_ENTITLEMENT_SERVER_URL_STRING` | 运营商 entitlement server HTTPS URL |
| `KEY_SHOW_VOWIFI_WEBVIEW_BOOL` | 是否需要用户通过 web portal 完成 VoWiFi 开通 |
| `KEY_WFC_EMERGENCY_ADDRESS_CARRIER_APP_STRING` | 处理紧急地址流程的系统/运营商组件 |
| `KEY_IMS_PROVISIONING_BOOL` | 是否需要后台 IMS provisioning |

Android 文档明确说明，北美运营商通常需要 WebView，用于条款和紧急地址输入。

### 4.2 AOSP 代码入口

AOSP 提供两部分参考实现：

- [service_entitlement TS.43 library](https://android.googlesource.com/platform/frameworks/libs/service_entitlement/+/refs/heads/main/)
- [ImsServiceEntitlement client app](https://android.googlesource.com/platform/packages/apps/ImsServiceEntitlement/+/refs/heads/main/)

对 SimAdmin 最有参考价值的文件包括：

- [ImsEntitlementApi.java](https://android.googlesource.com/platform/packages/apps/ImsServiceEntitlement/+/refs/heads/main/src/com/android/imsserviceentitlement/ImsEntitlementApi.java)
- [WfcActivationController.java](https://android.googlesource.com/platform/packages/apps/ImsServiceEntitlement/+/refs/heads/main/src/com/android/imsserviceentitlement/WfcActivationController.java)
- [WfcWebPortalFragment.java](https://android.googlesource.com/platform/packages/apps/ImsServiceEntitlement/+/refs/heads/main/src/com/android/imsserviceentitlement/WfcWebPortalFragment.java)
- [Ts43VowifiStatus.java](https://android.googlesource.com/platform/packages/apps/ImsServiceEntitlement/+/refs/heads/main/src/com/android/imsserviceentitlement/ts43/Ts43VowifiStatus.java)

### 4.3 AOSP 实际行为

`ImsEntitlementApi`：

1. 读取运营商 entitlement server URL。
2. 查询 VoWiFi/VoLTE/SMSoIP entitlement。
3. 缓存短期 authentication token 和配置版本。
4. 处理 token 过期、`Retry-After` 和重新完整认证。
5. 从 VoWiFi 响应中提取标准 `ServiceFlow_URL` / `ServiceFlow_UserData`，并兼容旧命名 `ServerFlow_URL` / `ServerFlow_User_Data`。

`WfcActivationController`：

1. 查询 entitlement 状态。
2. 如果 VoWiFi 已 entitled，完成激活。
3. 如果条款或地址数据缺失，显示运营商 websheet。
4. websheet 完成后重新查询状态，而不是仅凭页面关闭就认为成功。

`WfcWebPortalFragment`：

- 使用 WebView 加载运营商页面。
- `ServerFlow_User_Data` 存在时使用 POST。
- 向页面注入名为 `VoWiFiWebServiceFlow` 的 JavaScript bridge。
- 页面调用 `entitlementChanged()` 表示流程完成。
- 页面调用 `dismissFlow()` 表示取消或失败。

这证明标准实现是“设备端承载运营商页面”，不是设备自行猜测表单字段并直接写地址。

SimAdmin 的实现边界：创建 operation 后返回同源的一次性 `launch_url`。该页面只在
operation 仍处于 pending 且未过期时读取加密 secret store，并对标准
`application/x-www-form-urlencoded` 的 `ServiceFlow_UserData` 生成 POST form；JSON
接口不会返回 user data、token 或 cookie。operation completion 使用独立随机 nonce，不复用
运营商的 `ServiceFlow_UserData`。运营商若返回非 URL-encoded/二进制 user data，
页面会拒绝猜测并退回人工打开 URL。页面关闭本身仍不改变 entitlement 状态，必须通过运营商
提供的 callback bridge（或人工 callback API）后重新 query。

### 4.4 对 SimAdmin 的复用边界

SimAdmin 已经具备 USIM AKA/APDU 与 EAP-AKA 基础，但 TS.43 不能直接复用 IKEv2 内的整个 EAP-AKA 状态机。推荐只复用最底层能力：

- 对明确 `line_id` 的 SIM 执行 RAND/AUTN challenge。
- 返回 RES/CK/IK/AUTS 等受控结果。
- 不暴露 Ki 或其他长期密钥。

TS.43 HTTP challenge、authentication token、XML 文档和 websheet 状态机应当是新的独立协议层。

如果参考或移植 AOSP 代码，必须遵守其文件头所示许可证和 attribution 要求。

## 5. Apple/iPhone 的公开行为

[Apple 的 Wi-Fi Calling 支持文档](https://support.apple.com/en-us/108066) 给出的用户流程是：

```text
Settings
→ Cellular
→ 多 SIM 时选择目标 line
→ Wi-Fi Calling
→ Enable
→ 根据运营商要求输入或确认 emergency address
```

Apple 同时说明：

- 蜂窝网络可用时，紧急呼叫优先使用蜂窝网络。
- 蜂窝网络不可用时，紧急呼叫可能使用 Wi-Fi Calling。
- 设备位置可能用于协助应急响应，即使用户没有主动打开普通 Location Services。

Apple 的公开文档没有披露 iOS 内部使用的 entitlement 版本、EAP-AKA HTTP 报文或 websheet callback。因此可以确认“iPhone 是入口和客户端”，但不能仅凭公开资料断言所有 iPhone 运营商都使用与 AOSP 完全相同的 TS.43 实现。

SimAdmin 不应尝试伪装成 Apple 私有客户端。更合理的策略是：

1. 优先实现公开 TS.43。
2. 从合法取得且可审计的 carrier config/IPCC 中只提取事实。
3. 对 Apple 专用或私有流程标记 `unsupported`，除非已有可靠协议证据和测试订阅。

## 6. 美国运营商公开说明的差异

### 6.1 Verizon

[Verizon Wi-Fi Calling FAQ](https://www.verizon.com/support/wifi-calling-faqs/) 说明：

- 激活 Wi-Fi Calling 时必须确认、更新或输入美国地址。
- 地址决定 911 呼叫的路由，并可在呼叫者无法报告位置时提供给 emergency services。
- 如果蜂窝网络不可用，Wi-Fi Calling 的 911 呼叫使用 registered address 路由。
- 用户变更位置后应更新 emergency address。
- iOS 首次启用还可能要求连接美国境内 Verizon 网络。

### 6.2 AT&T

[AT&T Wi-Fi Calling 支持文档](https://www.att.com/support/article/wireless/KM1063258/) 说明：

- E911 地址可在开启 Wi-Fi Calling 的同一设置界面更新。
- 911 呼叫尽量优先使用蜂窝网络。
- Wi-Fi Calling 承载 911 时，会结合设备和 Wi-Fi 网络的位置数据进行路由。
- 如果位置不确定，则使用登记的 E911 信息。

### 6.3 由此得到的结论

不同运营商并不保证采用完全相同的定位和路由策略：

- registered address 仍然重要，并且经常是开通条件。
- 现代网络还可能结合设备位置、Wi-Fi 网络位置或其他动态定位。
- 某运营商允许账户网站/App 更新地址，不代表 entitlement websheet 可以被普通浏览器直接复用。
- 某运营商能在 iPhone 上开通，不代表第三方 Linux 客户端不会遇到设备类型、IMEI、User-Agent、区域或白名单限制。

## 7. 地址保存在哪里

### 7.1 运营商远端

地址的权威副本通常在运营商 E911/provisioning 系统中，与订阅或无线线路关联。实际远端主键可能是 IMSI、MSISDN、内部 subscriber ID 或其组合，不应假设运营商以 ICCID 为唯一键。

SIM 的主要作用是：

- 证明当前设备持有相应订阅。
- 参与 EAP-AKA challenge-response。
- 让运营商把 entitlement 操作关联到正确线路。

地址本身通常不会写回 SIM 文件系统。

### 7.2 SimAdmin 本地

SimAdmin 的本地绑定仍建议使用：

- 物理 SIM：ICCID。
- eUICC profile：EID + 当前 profile ICCID。

这是本地防串卡设计，不代表远端协议也使用这些字段。

推荐支持两种存储模式：

#### Websheet 模式

用户直接在运营商页面输入地址。SimAdmin 不保存完整地址，只保存：

- provider/profile ID。
- entitlement 状态。
- 最近成功确认时间。
- 运营商返回的非敏感 reference。
- 是否需要重新确认。

#### Native provider 模式

只有在某运营商已验证原生地址 API 时，SimAdmin 才显示自己的地址表单。用户明确保存时，可把地址作为 SIM override 的高敏感字段；运营商返回的状态和 token 仍放到独立 state/secret store。

不论哪种模式，后台轮询都不能修改用户 override 文件。

## 8. 当前 SimAdmin 的实现基础与缺口

### 8.1 已有基础

- `backend/src/connectivity/modems/ims/vowifi/qmi_uim.rs`：USIM authenticate APDU 与响应解析。
- `backend/src/connectivity/modems/ims/vowifi/eap_aka.rs`：EAP-AKA packet 与 key material 处理。
- `backend/src/connectivity/modems/ims/vowifi/profile_import.rs`：从部分 CarrierConfig/IPCC XML 提取 entitlement URL。
- `backend/src/connectivity/modems/ims/vowifi/profile_record.rs`：E911 policy 元数据。
- `reqwest + rustls`：可作为 HTTPS client 基础。
- `LineRuntimeRegistry`：可把 entitlement operation 隔离到具体线路。

### 8.2 尚未实现

- TS.43 HTTP client。
- HTTP EAP-AKA challenge/authentication state machine。
- TS.43 XML parser 和 response model。
- entitlement token/config-version store。
- `AddrStatus/TcStatus/ProvStatus` 状态机。
- 安全 websheet operation 与 callback。
- 按 `SimBindingKey` 的 E911 状态持久化。
- entitlement endpoint 的可信 allow-list/evidence。
- 紧急 IMS 呼叫、emergency registration 和 fallback。

### 8.3 catalog 的现实限制

当前随 release 使用的 carrier catalog 虽然能标记部分美国 profile 的 emergency capability，但没有足够的 provider/entitlement URL 可以覆盖常见美国 SIM。

因此，完成 TS.43 engine 后还需要：

1. 从可信 CarrierConfig、IPCC 或运营商资料导入 endpoint。
2. 保存 endpoint 来源和 evidence。
3. 针对实际持有的美国 SIM 做非紧急 entitlement 查询验证。

不能通过 MCC/MNC 推导或猜测 URL。

## 9. 推荐的 SimAdmin 架构

### 9.1 模块边界

```text
backend/src/connectivity/entitlement/
  mod.rs
  ts43/
    client.rs
    eap_aka.rs
    request.rs
    response.rs
    xml.rs

backend/src/services/e911/
  mod.rs
  model.rs
  registry.rs
  orchestrator.rs
  state_store.rs
  operation_store.rs
  providers/
    ts43.rs
    external_portal.rs

frontend/src/pages/sim/
  E911StatusPanel.tsx
  E911WebsheetDialog.tsx
```

`connectivity/entitlement` 只负责协议，`services/e911` 负责按线路/SIM 编排、权限、状态与审计。VoWiFi `live.rs` 只消费最终 capability，不直接执行地址表单。

### 9.2 主要上下文

```rust
struct E911Context {
    line_id: String,
    sim_binding: SimBindingKey,
    catalog_profile_id: String,
    provider_id: String,
    entitlement_url: Url,
    modem_imei: Option<String>,
    custom_imei: Option<String>,
}
```

真正执行 SIM authenticate 前必须再次确认 `line_id` 当前仍绑定相同 `SimBindingKey`。

### 9.3 状态模型

```text
unsupported
unknown
unconfigured
querying
needs_terms
needs_address
needs_user_action
provisioning
provisioned
rejected
stale
temporarily_unavailable
```

状态来源必须保留：

```text
carrier_confirmed
carrier_declared
local_only
unknown
```

只有重新查询 entitlement 得到成功结果，才允许进入 `provisioned/carrier_confirmed`。

### 9.4 Provider 类型

```text
Ts43Provider
    标准 TS.43 query + EAP-AKA + websheet

ExternalPortalProvider
    引导用户使用运营商官方账户页面，完成后只重新查询状态

NativeVerifiedProvider
    仅用于已经确认的运营商原生 API

MetadataOnlyProvider
    只提示需要 E911，不执行网络请求
```

未知运营商默认使用 `MetadataOnlyProvider`，不能自动尝试其他运营商 endpoint。

## 10. Websheet 在浏览器项目中的难点

AOSP 使用的是受信任的原生 WebView，并向页面注入：

```javascript
VoWiFiWebServiceFlow.entitlementChanged()
VoWiFiWebServiceFlow.dismissFlow()
```

SimAdmin 前端运行在普通浏览器，不能假设跨域页面允许 iframe，也不能直接向跨域 popup 注入 JavaScript object。还可能遇到：

- `X-Frame-Options` 或 CSP `frame-ancestors`。
- Same-Origin Policy。
- 第三方 cookie 限制。
- websheet 只接受 POST，不接受裸 GET。
- 页面完成后只调用原生 JS bridge，不提供 redirect URI。

建议按以下优先级处理：

1. 运营商支持 redirect/callback URL：使用一次性 operation callback。
2. 运营商页面支持标准浏览器：使用新窗口 + 后端轮询 entitlement 状态。
3. 只能调用原生 JS bridge：需要单独的受控 WebView companion，不能用普通 iframe 假装兼容。
4. 无可靠交互方式：回退到官方账户页面或标记 unsupported。

不要用通用反向代理重写运营商页面。它容易破坏 TLS/cookie/CSP，并会让 SimAdmin 接触不必要的账户凭据。

## 11. API 建议

```text
GET  /api/ims/lines/{line_id}/e911/capability
POST /api/ims/lines/{line_id}/e911/query

GET  /api/ims/lines/{line_id}/e911/status
POST /api/ims/lines/{line_id}/e911/operations
GET  /api/ims/lines/{line_id}/e911/operations/{operation_id}
POST /api/ims/lines/{line_id}/e911/operations/{operation_id}/callback
POST /api/ims/lines/{line_id}/e911/operations/{operation_id}/cancel
```

仅 Native provider 额外提供：

```text
GET    /api/ims/lines/{line_id}/e911/address
PUT    /api/ims/lines/{line_id}/e911/address
DELETE /api/ims/lines/{line_id}/e911/address
POST   /api/ims/lines/{line_id}/e911/address/validate
POST   /api/ims/lines/{line_id}/e911/address/provision
```

普通状态响应不返回完整地址、IMSI、ICCID、IMEI、token 或 `ServerFlow_User_Data`。

## 12. 安全要求

### 12.1 Endpoint 与 SSRF

- 只允许 HTTPS。
- endpoint 必须来自 sealed catalog 中有 evidence 的 policy，或经过管理员明确审批。
- 禁止 localhost、IP literal、私网、link-local 和云 metadata 地址。
- DNS 解析和每次 redirect 后都重新检查目标地址。
- redirect 仅允许 provider host allow-list。
- 禁止关闭证书或 hostname 校验。

### 12.2 SIM 与 AKA

- 每次 challenge 都绑定明确的 `line_id + SimBindingKey`。
- 不允许退回第一张 SIM、默认 modem 或空 line。
- RAND/AUTN/RES/CK/IK/AUTS 不进入 INFO 日志、诊断响应或普通 tracing span。
- AKA 长期密钥永远不离开 SIM/UICC。

### 12.3 Websheet operation

- operation ID 随机、一次性、短 TTL。
- `ServerFlow_User_Data` 按 secret 处理并加密暂存。
- callback 需要 CSRF/state 校验。
- 地址流程中换卡时立即取消 operation。
- 页面完成后必须重新查询运营商状态。

### 12.4 PII

- 地址、ICCID、IMSI、EID、IMEI 不写普通日志。
- UI 列表只显示 `configured/provisioned/stale`。
- 完整地址仅在用户主动进入受认证编辑页时返回。
- 导出、备份和诊断包默认排除地址与 entitlement secret。

## 13. 分阶段实现顺序

### Phase A：只读 entitlement query

- 建立 `SimAkaProvider` 窄接口。
- 实现 TS.43 HTTP EAP-AKA。
- 解析 VoWiFi status、地址/条款/provisioning 状态。
- 仅查询，不打开页面、不提交地址。
- 使用 mock server 和公开 fixture 测试。

完成条件：能按线路查询状态，双卡不串线，日志无敏感材料。

### Phase B：安全 websheet operation

- 支持标准 `ServiceFlow_URL/ServiceFlow_UserData`，兼容旧资料中的 `ServerFlow_URL/User_Data` 命名。
- 建立短期 operation store。
- 支持已验证的浏览器 redirect/callback 或轮询模式。
- 页面完成后重新查询 entitlement。

完成条件：关闭页面、超时、换卡、重启和重复 callback 均有确定状态。

### Phase C：SIM 绑定的持久状态

- 按 ICCID 或 EID + profile ICCID 保存非敏感 entitlement 状态。
- token 进入独立 secret store。
- eSIM profile 切换使旧状态变 stale。

完成条件：同 PLMN 双卡、跨 modem 移卡、实体 eSIM profile 切换都不会读取错误状态。

### Phase D：运营商适配

- 从可信 catalog 增加 provider endpoint/evidence。
- 优先验证标准 TS.43 运营商。
- 对非标准运营商选择 external portal 或独立 adapter。
- 每个 adapter 配套脱敏 fixture 和非紧急实卡验证记录。

完成条件：没有 endpoint/evidence 的运营商明确返回 unsupported，不做猜测请求。

### Phase E：紧急呼叫能力

这一阶段独立于地址 provisioning，需另行实现和审阅：

- emergency number classification。
- IMS emergency registration/call routing。
- location 与 callback 语义。
- cellular/CS fallback。
- 运营商和监管要求的实验室验证。

不得把 Phase A–D 完成等同于紧急呼叫完成。

## 14. 测试方案

### 14.1 Unit

- TS.43 request/XML response parsing。
- EAP-AKA success、sync failure、token expired、retry-after。
- `AddrStatus/TcStatus/ProvStatus` 全组合。
- endpoint/redirect allow-list。
- 状态迁移和 operation TTL。
- PII redaction。

### 14.2 Integration

使用本地 mock entitlement server 覆盖：

- 已 provisioned。
- 需要条款。
- 需要地址。
- GET websheet。
- POST websheet。
- callback 成功、取消、重复、超时。
- token 过期后完整重新认证。
- 503 + Retry-After。
- 恶意 redirect、私网 URL 和超大响应。
- 操作期间 SIM 被拔出或换卡。

### 14.3 多线路

- 同 PLMN 两张卡返回不同 entitlement 状态。
- A 线 websheet 不能完成 B 线 operation。
- A 线换卡后旧 callback 失效。
- B 线查询不等待 A 线用户填写页面。

### 14.4 实卡非紧急验证

只有满足以下条件才执行：

- 用户拥有并授权测试该订阅。
- endpoint 与 provider 已确认。
- 使用运营商 sandbox，或由用户明确批准对自己的账户执行地址登记。
- 不记录完整地址和认证材料。
- 不拨打 911。

如果运营商没有 sandbox，只能验证 entitlement 查询和官方地址登记流程，不能自动化执行真实地址覆盖。

## 15. 完成标准

E911 地址 provisioning 可以标记完成，必须同时满足：

- 状态来自运营商回读，不是本地布尔值。
- websheet/native provider 至少有一个经过非紧急实卡验证。
- 同 PLMN 双卡与 eSIM profile 切换不串状态。
- 自动轮询不改写用户 override。
- endpoint、redirect、token 和地址符合安全要求。
- UI 明确区分“地址已登记”与“紧急呼叫已验证”。

紧急呼叫支持必须另行验收，不能通过普通号码、SIP REGISTER 或地址登记成功推断。

## 16. 推荐决策

结合当前 SimAdmin 架构，建议采用：

```text
第一目标：TS.43 read-only entitlement query
第二目标：标准 websheet operation
第三目标：按实际持有的美国 SIM 增加 provider evidence
第四目标：必要时增加 Native provider
最后单独处理 emergency calling
```

不建议第一版就在本地复制完整地址表单。标准 websheet 模式可以让地址直接进入运营商页面，减少 PII 存储和错误地址过期风险。

## 17. 公开资料

1. [GSMA TS.43 Service Entitlement Configuration](https://www.gsma.com/newsroom/gsma_resources/ts-43-service-entitlement-configuration/)
2. [Android IMS service entitlement](https://source.android.com/docs/core/connect/ims-service-entitlement)
3. [AOSP service_entitlement library](https://android.googlesource.com/platform/frameworks/libs/service_entitlement/+/refs/heads/main/)
4. [AOSP ImsServiceEntitlement app](https://android.googlesource.com/platform/packages/apps/ImsServiceEntitlement/+/refs/heads/main/)
5. [AOSP ImsEntitlementApi](https://android.googlesource.com/platform/packages/apps/ImsServiceEntitlement/+/refs/heads/main/src/com/android/imsserviceentitlement/ImsEntitlementApi.java)
6. [AOSP WFC activation controller](https://android.googlesource.com/platform/packages/apps/ImsServiceEntitlement/+/refs/heads/main/src/com/android/imsserviceentitlement/WfcActivationController.java)
7. [AOSP WFC web portal](https://android.googlesource.com/platform/packages/apps/ImsServiceEntitlement/+/refs/heads/main/src/com/android/imsserviceentitlement/WfcWebPortalFragment.java)
8. [Apple：Make a call with Wi-Fi Calling](https://support.apple.com/en-us/108066)
9. [Verizon Wi-Fi Calling FAQs](https://www.verizon.com/support/wifi-calling-faqs/)
10. [AT&T：Stay connected with Wi-Fi Calling](https://www.att.com/support/article/wireless/KM1063258/)

运营商页面、支持范围和协议版本可能随时间变化。实现时应将 endpoint、协议版本、来源证据和验证时间作为 catalog 数据保存，而不是写成长期不变的代码常量。
