# 无忧行 APK 与原生通信协议逆向成果总结

> 整理基线：2026-10-28；对象为本项目持有的无忧行 APK 样本及其中的 ARM64 `libmtc.so`。
> 已核验源码：`2986a3d8b1283d8aaa428d252ab83098a1f311bf`；对应手册检查点：`9901cd1f2fa0cd43a2211914539affa9c185bc08`。
> 本文汇总逆向事实、已落地实现及验证边界，不包含账号凭据、SDK 内置密钥、私钥或可直接重放的服务请求。最新项目状态仍以 [HANDOFF](HANDOFF.md) 为准。

## 1. 核心结论

1. **无忧行托管不是直接复用 SIM 的 IMS/VoWiFi 注册。** App 使用 Juphoon/Justalk SDK；已还原的路径是应用账号登录、原生 ARC/Common RPC、SipGateway 托管、CallEx 来电及 Endpoint/Mpath 媒体。发现 `SipGateway` 名称不等于证明可以对某个公网 SIP SBC 直接注册。
2. **凭据至少分为 App API 会话、电话 SDK 登录、原生 ARC 会话和托管网关四个层次。** 它们的 token、会话选择器、应用配置和 SIP 密码不能互换。
3. **基础入站链路已在 Rust 后端落地。** 包括显式参数导入、登录/托管、接短信与历史/通知、来电应答/挂机、双向音频和 Trunk，以及原运行清理、有界重连和带标记的退出进程恢复。
4. **原生 `RseEx.SDP` 有 raw 与压缩编码两条路径。** 已追踪 Base64 检测、解码/解压和按邀请格式回复；后端实现 raw 或 zlib＋Base64，Trunk/API 始终使用 raw SDP。
5. **静态分析不等于服务验收。** 已执行的是 SimAdmin 与隔离协议对端的测试，不是运行原 Android SDK，也不是连接真实无忧行服务。当前仍没有真实账号、部署、账单或长期在线互通验收结论。
6. **仍有可继续研究的未知分支。** 自动续期、完整 Locator/NetStream、其他媒体组合及远端设备并发语义尚未闭合；不能把“尚未找到引用”写成“代码不存在”，也不能把已实现子集写成完整 SDK 的等价重实现。

### 证据如何解读

| 标记 | 含义 | 不能据此推出 |
| --- | --- | --- |
| 静态已确认 | 在实际 APK/DEX、原库字节、IDA 反编译或反汇编中找到调用/数据流 | 服务端当前接受该请求、部署选择该模式 |
| 实现并隔离验证 | 生产 Rust 代码及相关测试已在指定 Actions 源码提交执行 | 原 Android SDK 与真实服务已互通 |
| 部分还原/待确认 | 只有注册名、部分结构、分支或缺失的运行时参数 | 可以任意填默认值、推断授权或直接启用 |

## 2. 分析样本与方法

### 2.1 实际样本指纹

| 对象 | 大小（字节） | SHA-256 |
| --- | ---: | --- |
| `com.cmi.jegotrip.apk` | 132422053 | `9874f23c3cfde41cecfc23b67b6da7d81e161a81cc875d05fc33483b89478a1e` |
| APK 外层 `classes.dex` | 47202304 | `6fc989494a528b43bb060e60b941cd44c01c812e3a3112bddeb270c78da946e4` |
| `lib/arm64-v8a/libmtc.so` | 24598088 | `57086cd624d8227dc1b1b64a63d7c8b2258f87b261779b681a9bfd306ad9d2e8` |

外层 `classes.dex` 中重新定位并校验了 **7 个嵌套 DEX**，而不是只分析外层少量类。下列偏移均相对于该外层 DEX，不是 APK ZIP 文件偏移或运行时地址：

| 偏移 | DEX 大小（字节） | 本轮用途 |
| --- | ---: | --- |
| `0xC000` | 9246612 | 全样本检索与交叉核对 |
| `0x8E0000` | 9908524 | 主要 App 业务、凭据、托管逻辑；8567 个类 |
| `0x1254000` | 8690892 | 全样本检索与交叉核对 |
| `0x1AA0000` | 9252896 | Juphoon/Justalk Java 封装及 JNI 声明 |
| `0x2374000` | 9812524 | 全样本检索与交叉核对 |
| `0x2CD0000` | 82748 | 全样本检索与交叉核对 |
| `0x2CE8000` | 39192 | 全样本检索与交叉核对 |

第三方《无忧行_8.8.4.0_逆向分析报告》描述的 APK 大小为 123106470 字节、SHA-256 为 `a58bac608095819752010579ea4c0e5883bcb01cdb5ab5e903bdf4f316981f33`，**与本项目 APK 不是同一字节样本**。报告只用于提供线索，不能直接套用其偏移、常量或服务结论。

### 2.2 分析边界

- DEX 使用已有 Androguard 4.1.4 只读解析，未执行 APK 或反序列化不受信任的 Java 对象。
- IDA 只读使用用户已打开的原 `libmtc.so`；核对文件 SHA-256 与基址 `0`，未 patch、rename、修改类型或保存 IDB。
- 用 ELF 重定位、FDE 函数边界和 ARM64 指令交叉核对反编译，避免把合并的异常处理入口或虚表中段误认成函数。
- Rust、前端及编译后的协议对端仅在 GitHub Actions 执行；本机仅做静态/mock 检查与文件/制品核验。
- 所有 native 地址仅是**上述样本的证据定位**，不是生产程序硬编码地址，也不能移用于不同版本。

## 3. 分层架构与正常托管流程

```text
App 账号 / HTTP API
  ├─ App API 会话 token、请求签名、可配置请求体加密
  ├─ 获取电话登录 token：csb/token
  └─ 获取托管状态 / SIP 网关密码
          │
          ▼
JustalkManager → JCClientImpl → MtcEngine → JNI → libmtc
          │
          ▼
显式原服务器 / TLS / ARC 会话
  ├─ 可选 getAppConfigs → AccountEntry 配置与引用
  ├─ loginSession → 原生会话认证结果
  └─ swapCalls → 托管请求、入站调用及回执
          │
          ├─ JCAccount.updateStatus(4) → SipGateway 注册托管
          ├─ Sip.Request → 短信通知 / 文本历史
          └─ CallClientEx → Inform / Accept / Release
                                  │
                                  ▼
                         Endpoint TCP / Mpath
                                  ↕
                      SimAdmin 本地 UDP RTP / Trunk
```

这是逻辑分层图；**HTTP API、SDK 配置、Common RPC、TLS、Router 和媒体不是一种协议**。某一层的连接或 ACK 成功，不能替代下一层的成功。

正常停止需要区分：托管注销 `status=3` 的原请求结果、SDK `logoutSession` 的结果、原 actor/consumer/媒体/Trunk 的实际退出，以及本地持久运行记录的释放。不能只看到 socket 关闭就宣称所有清理成功。

## 4. 凭据、配置与 token 续期

### 4.1 不能混用的字段

| 材料 | 静态来源/使用位置 | 用途与限制 |
| --- | --- | --- |
| App API 会话 token | HTTP 配置或已登录 `User` 对象 | 认证 App HTTP 请求；不是电话 SDK 登录 token |
| 电话登录 token | `csb/token` 的 `body.token`，原本地偏好；另一路为 `CallUserInfo.cstToken` | `JCClient.login` 第二参数；原生登录 metadata 的 `pwd` |
| SIP 网关密码 | `VoipStatus.sipPassword`，经 `decryptThreeDESECB` 处理 | 托管 `SipPwd`；不能直接用缓存密文或电话 token 替代 |
| SDK application key | `JCClient.create` 的原配置 | 标识 SDK 应用上下文；不是用户授权或免认证凭据 |
| 原选 SDK session ID | 原 SDK 会话配置/选择流程 | `loginSession`、`swapCalls` 的会话选择参数，不是 auth token |
| ARC 返回的 session token | 成功 `loginSession` 回包 | 后续 Common RPC metadata 的 `token`；与登录 `pwd` 分离 |
| Client / Common 配置解码 key | 两个独立 PBE 上下文 | 仅解包 `ENC(...)` 配置；不能用作 TLS 信任、账号密码或授权证明 |
| 原服务器、root、authority、设备标识 | 原客户端配置 | 保留部署信任与设备绑定；不从 HTTP 域名、号码或属性名称推导 |

### 4.2 已找到的活跃电话 token 获取链

```text
DeepLinkUtil.getPhoneLoginToken(Context)
  → SysApplication.getAreaCodeMobileNum()
  → CallLogic.getLoginToken(accountId, callback)
  → JegoTripApi.GET_PHONE_LOGIN_TOKEN
       路径：service/voip/v1/csb/token
       请求字段：accountId
  → DeepLinkUtil$4.onResponse
       code == "0" → body.token
  → LocalSharedPrefsUtil.setPhoneToken
       preference：wyx_phone_login_token
       key：wyx_phone_login_token_key
  → 后续 JustalkManager.login → JCClient.login(identity, phoneToken)
```

初始化、App 登录后、部分前台处理以及空 token 的交互恢复路径存在直接调用。因此这里不仅找到了一个 URL，还确认了调用、响应解析、存储和消费链。

**但获取成功回调只存 token：**没有在该回调里解析 TTL、注销/重启已托管会话或原子替换正在使用的原生认证。不能把前台代码中的时间常量当成服务端有效期，也不能把获取新值等同于活动通话已完成续期。

### 4.3 `refreshToken` 目前只能确认到 helper

`JegoTripApi` 还声明 `service/voip/v1/refreshToken`；`CallLogic.requestDelayTokenValid(callback)` 使用普通 `HttpRequest.request` 发送空正文。

本轮扫描的结果是：

- 在应用 DEX 全部 8567 个类的指令中，只找到 URL 字段初始化与 helper 自身，没有找到直接调用者。
- 没有找到该 helper 的专用响应消费者、TTL 解析或更新电话 token 的调用链。
- 相关精确符号仅位于相应嵌套 DEX；未在扫描的 native/JS bundle 成员中找到这些符号。

**这是“未找到静态调用链”，不是证明反射、动态代码或服务端功能绝不存在。**尚不能确定该接口延长哪一种有效期、是否轮换凭据、是否已停用，或是否会影响现有 SDK 会话。

当前后端仍声明 `renewal = manual_reimport_until_renewal_verified`。要实施自动续期，还需要经单独授权取得真实响应结构、TTL/轮换行为、失效/撤销错误、设备绑定及旧会话交接语义；不能复制内置常量或匿名 token 冒充账号授权。

### 4.4 HTTP 与 SIP 密码来源补充

- `HttpRequestUitls.main` 添加 timestamp/signature；token 后缀来自 HTTP 配置、App 用户或匿名配置的不同分支，`csb/token` 有专门的后缀处理。
- `csb/token` 调用 `requestWithEncrypt`，请求体是否加密由 `AppConfig.isEntryBody()` 决定；`requestDelayTokenValid` 走普通请求方法。这里的“普通正文”不等于证明网络连接无 TLS。
- SIP 密码缺失时，`queryOpenVoipStatus` 对应 `service/voip/v1/openVoipStatus`，相关回调解析 `body.sip_password` 和 `body.status` 并保存 `VoipStatus`。未据此确认周期性的网关密码轮换协议。
- Java/HTTP 的签名、加密或 Android TLS 处理不证明原生 ARC 的安全模式，更不证明可以跳过服务端登录或托管开通。

## 5. 原生 Common/ARC、配置与传输

### 5.1 二进制序列化规则

已确认并实现的基本规则包括：

- 嵌套流以 `StreamData=0` 开始；支持的方法参数前缀为 `00 00 01 00 00`，含义是 **流类型 0、BE16 版本数量 1、BE16 方法版本 0**，不是 major/minor 版本对。
- 整数按对应宽度的大端编码；字符串/原始正文是长度前缀字节串，不能把二进制 metadata 一律当 UTF-8 文本。
- Common 字符串键按“长度优先，同长度再按字节序”排序；批次 request/result map 的键是有符号 `i32`。
- 请求关联 ID、batch sequence 和 native call ID 是不同命名空间。native call ID 必须保留完整 BE64。
- 外部 RPC result code、内部 boolean、返回属性和 Common footer 分别检查；数据可解码不等于业务成功。

### 5.2 主要方法及已确认用途

| 原生方法 | 用途 |
| --- | --- |
| `getAppConfigs.AccountEntry.Account` | 使用原应用配置上下文取得账号入口及属性 |
| `loginSession.AccountEntry.Account` | 原 SDK 会话登录；返回用户标识、ARC token 和属性 |
| `swapCalls.AccountEntry.Account` | 交换批次请求、结果和 ACK 状态 |
| `logoutSession.AccountEntry.Account` | 原 SDK 会话注销 |
| `updateStatus.SipGatewayPub.SipGateway` | 托管注册/注销；`SipFlag` 分别为字符串 `4` / `3` |
| `recvOnlineMessage.MessageReceiver.Client` | 原 receiver 上的消息类型、属性和原始正文 |
| `onInvited.CallClientEx.CallEx` | 原来电邀请 |
| `onReleased.CallClientEx.CallEx` | 原通话释放通知 |
| `onAccepted.CallClientEx.CallEx` | 原通话接受通知 |
| `inform.CallServerEx.CallEx` | 对原通话提交 Inform |
| `accept.CallServerEx.CallEx` | 应答原通话 |
| `release.CallServerEx.CallEx` | 释放原通话 |
| `connect.Endpoint.Endpoint` | 使用原 Accept 返回的 `content` 建立媒体 Endpoint |

`updateStatus` 的参数为原 `current_user_uri` 加属性 map；map 包括 `SipFlag`、`AuthnamePrefix`、`ChannelId`、`DeleteIntCode`、`SipNumber`、`SipPwd`。App 侧的 `JCAccount.updateStatus(4, ...)` 与这个 wire 层字段形状应分开理解。

### 5.3 配置解包与 bootstrap

- `ENC(...)` 已追踪到独立 Client 和 Common reference PBE 上下文。已实现子集使用 Base64 包装的 `salt[8] || DES-CBC ciphertext`，MD5 派生共 1000 轮，前 8 字节为 DES key、后 8 字节为 IV，块填充为 PKCS#5。
- 这只是**兼容旧配置格式**，不是 TLS/SIP/AKA 算法回退；该 CBC 格式没有认证 MAC，解密/填充成功不代表可信身份或服务授权。
- 引用语法已区分 identity/facet、adapter/locator 与显式 endpoint；解析到间接引用不等于已经实现其解析服务。
- bootstrap 的原成功回包和非空 `AccountEntry` 是下一步前提。`DomainName` 是原 map 值，`DomainId`、`AppId`、`CC` 按 Client 上下文处理；缺少 CC 不继承旧值。
- SimAdmin 的可选 bootstrap handoff 要求单独导入目标账号 TLS 信任；原 source actor 实际 Join 后才能建立新连接，不把返回字符串当成新的 root/SNI/authority，也不重定向现有活动 IO。

### 5.4 TCP、Router、TLS 与 Common 边界

| 层 | 已还原的关键事实 | 当前限制 |
| --- | --- | --- |
| PacketSender TCP | `BE32(packet.len())` 后接原 `packet`；正确处理流的拆包/合包 | 只用于已证明的 PacketSender 路径，不推广为所有 native 传输 |
| Router | 8 字节头、序号/时钟/分片字段、路由数据和部分控制包 | 地址保持协议数据；不从逻辑路由猜 OS 端点或媒体权限 |
| TLS | 原 NetSsl 使用 memory BIO；TLS plaintext read 不保证一读一包 | SimAdmin 要求显式 root/authority、证书验证与固定预算，不采用宽松 verifier |
| Common | 已支持模式的请求/结果流可由长度/计数自定界；mode 3 有原分片规则 | 不能凭 TLS read 边界分包，也不能擅加内层 BE32 长度 |

原 Router 的部分序号 bitmap 只做统计；Mpath 的 bitmap 则参与丢弃重复媒体数据。两个机制不能因都是 bitmap 就互相代用。完整拥塞控制、其他代理/传输、安全变体不在当前支持承诺内。

## 6. 短信接收、批次回执与长期收件

### 6.1 从原 receiver 到短信文本

```text
精确 receiver kind：Sip.Request
  → MtcSipGatewayMsgReciver
  → MtcSmgwMessageRecvNotification
  → JCMessageChannel / App 短信历史
```

静态证据确认：native 复制正文时保留原长度，经 JSON 字符串通知到 Java；这条路径不是 GSM PDU 解码，也不是自动 Base64 解码。

| 数据 | 语义 |
| --- | --- |
| 原始正文 | 只有符合已证文本子集时才投影为短信；看似十六进制/Base64 的文本仍按原文本保留 |
| `Sip.Message.From` | 原发送方信息；不从网关引用或当前账号补猜 |
| `Sip.Message.Time` | 可选的十进制 Unix 毫秒时间；缺失时明确标注接收时间来源 |
| `Sip.MessageGw.Oid` | 独立网关引用，不是发送方、全局消息 ID 或新授权 |
| Java messageId | 可由本地随机 UUID 生成，不是服务端跨连接去重 ID |

后端对精确原 receiver、owner/scope、严格 UTF-8、发送方和时间格式进行校验。不认识的 kind、编码或异常字段保持未分类，不以“像文本”就写入 SMS。

### 6.2 DIRECT 与 batch 的确认不能混淆

- **DIRECT：**已还原原 `__saveCallId` 的双 180 秒 magic 窗口。SimAdmin 保留原请求关联，只有本 actor 的实际响应写入后才产生可回收依据；`direct_submitted` 不等于远端应用已确认。
- **batch：**六个 BE32 字段依次表达本端 incarnation、next index、history bitmap、已观察对端 incarnation、对端 next index、对端 bitmap；后面是请求和结果 map。
- 批次 ACK 位指向 `ack_next_batch - 1 - bit`。只有精确 ACK 了实际承载结果的 epoch，才能退休该结果；`batch_queued` 或外层交换成功不能替代这个证明。
- 原 batch 有 32 个历史 epoch、45 秒保留及 2048-bit 请求去重结构。项目对 before-base、会话变化和错误采取保守处理，不照搬会导致旧业务重新投递的重置行为。

### 6.3 项目中的耐久性设计（不是 Android 原实现）

- raw 入站、SMS 投影、事件及通知计划在同一事务提交后，才回复原消息票据。
- 归属由 `line_id + revision + run_nonce` 与原连接保持；不按接收时的“当前账号”重绑旧数据。
- DIRECT 与 batch 各保最近 128 条可回收的完成副本；保留完整 SMS 历史和未分类/pending 数据。批次稀疏退休记录限制为每账号 2048、全局 8192。
- GC 失败不重放已经提交的 DIRECT ACK 或已经确认的 batch result；SQL 失败保留原义务。
- 已有测试覆盖同一个原运行接收 600 条 DIRECT/600 条 batch 短信及 2300 序号滚动，但**不承诺无限磁盘、跨运行 exactly-once 或所有编码均可接收**。
- 通知保留原目的地指纹和发送状态；目的地被替换、消息删除或发送结果不确定时，不改投或盲目重发。删除是逻辑清除，不是对 SQLite/WAL/备份进行物理擦除。

## 7. 来电控制、媒体与压缩 SDP

### 7.1 原通话与 RPC 关联

- `onInvited` 参数为 BE64 native call ID、peer 字节串、属性 map；`onReleased/onAccepted` 为 BE64 ID 加 map。
- `Inform/Accept/Release` 必须关联原 receiver/agent、owner、连接、通话 ID 和原请求票据。
- 邀请 receipt、Inform 成功、Accept 成功和媒体建立是不同事件；SDK 报“已接受”不能直接标记 `media_active`。
- 原 Accept 结果的 `endpoint`、`content` 用于后续 `connect.Endpoint.Endpoint`；`content` 是该通话的独立数据，不是 phone token、SIP 密码或可任意推导的 URI。

### 7.2 已实现的媒体路径

原 native 在相应 raw-ARC listener/factory 未安装时存在 Mpath 路径，使用原 Endpoint 连接承载媒体。项目实现的是显式 Endpoint TCP/TLS 控制＋TCP-Mpath 媒体，与本地 UDP RTP 的双向转发。

已还原 Mpath 的 BE32 packed header：2-bit 分片类型、2-bit path、14-bit sequence 与压缩的 tick；序号模 16384。1～2918 字节 payload，小于等于 1459 字节单包，更大则分成两片；channel 字节只在整包或第一片。接收端按原 path/相邻序号去重、合片，再分发 channel。

**不能将 `Client.Host` / `Client.ArcPort` 当作 OS UDP/RTP 地址。**它们可能是逻辑 Router 地址/监听端口。本地音频端口必须真实绑定，peer 与 payload 按原通话协商检查。

支持单音频 `RTP/AVP`、`sendrecv` 与匹配编码的 RTP passthrough；Trunk 侧采用 RTCP-mux，支持已实现的编码/telephone-event 子集，包括 Opus passthrough，**不提供转码或浏览器 WebRTC 音频**。plain media 必须显式 opt-in，信令 TLS 不因此降级，也不能据 TLS 信令推断全部媒体已加密。

### 7.3 raw / zlib＋Base64 的选择与镜像

原库证据链：

| 位置 | 已确认行为 |
| --- | --- |
| `0x613140` / `0x6132A0..0x6132D8` | 读取邀请 `RseEx.SDP`，保存并调用格式检测 |
| `0x5FA164` → `0x251958` | 根据完整 Base64 字符语法设置 call 对象 `+1144` 标记 |
| `0x60ED60` | 标记清零读 raw；标记置位先 Base64 解码，再交给 inflater |
| `0x252E1C` / `0x252BCC` | Base64 输入与解码，区分标准/URL-safe 字母表 |
| `0x4E3C48` / `0x4E3D68` | 分别调用 zlib `inflate` / `deflate` |
| `0x5FA47C` | 根据原邀请标记选择 raw 回复或压缩后编码 |
| `0x61F894` / `0x253038` | 回复使用标准 Base64 字母表和 padding |

当前实现保留每个邀请的格式：

```text
native 邀请 raw --------------------------┐
                                        ├→ 严格 SDP 检查 → API/Trunk raw SDP
native 邀请 Base64 → zlib 解压 -----------┘

API/Trunk 应答 → 本地真实端口 SDP
                ├─ 原邀请 raw：native Accept 发送 raw
                └─ 原邀请压缩：native Accept 发送 zlib＋标准 Base64
```

实现设置独立 wire 大小限制与 **16 KiB 解压输出上限**；拒绝混合字母表、错误包装、超限膨胀和已检查的尾随输入。解码后的 `a=crypto`、fingerprint/setup、ICE/candidate、BUNDLE/group 等仍由原严格媒体策略拒绝，不因支持压缩而获得额外媒体权限。

该成果是原生调用路径的静态还原与当前普通 zlib 包装实现的隔离验证，**不是原 SDK 全部压缩上下文/初始化方式或真实服务 wire 的逐字节互通证明**。若实网样本需要额外字典、流状态或其他变体，仍须独立核对，不能把标准 zlib 测试数据当作实网抓包。

## 8. SRTP、DirectConnect、Locator 与 NetStream

### 8.1 当前 APK 的显式设置

- `JustalkManager.onLogin(true, ...)` 调用 `closeAudioSrtp()`；SDK 封装将 SRTP crypto type 置为 `0`、RTP authentication 置为 `false`，同时关闭若干音频特性，再单独应用 RED/FEC 配置。
- `MtcEngine.login` 显式调用 `Mtc_CallDbSetDirectConnectEnable(false)`。
- `setClientConfig` 写入显式 `config_key_server_address`，再传到 native network 配置；服务器来源含运行配置/用户设置/环境分支，不在本文公布内置部署值。
- 本次未在已扫描的 App 调用链中找到 `SRTP_ENABLE` getter 的使用；其他 IM/订单业务的 `P2P` 字符串不能用作语音 ICE 的证据。

这些证明的是**当前样本的设置调用**，不能排除其他 native 初始化、后续配置或未来 SDK 版本的行为。后端没有实现 SRTP、ICE/BUNDLE/P2P；`RseEx.CryptoKey` 只是已发现的可选 callback-hook 数据，不能直接当作已确认的 SRTP 密钥交换协议。

### 8.2 Locator：方法存在，完整运行语义未闭合

找到 `locateAdapter.Locator.Common`、`locateCategory.Locator.Common`、`locateObject.Locator.Common` 等名称及注册路径；`0xE816AC` 的名称/整数登记不等于 request serializer 或响应 schema。

App 显式设置 server address，**不等于证明 native 内部永远不会解析间接对象引用**。完整 Locator 请求/响应、缓存更新、失效与路由授权仍待还原；项目仍要求明确的支持路径，不自动猜默认 locator。

### 8.3 NetStream / Channel：已有局部结构，不是完整客户端

已有证据不只有类名：早期 ELF/IDA 记录已定位 `Common::NetStreamI` 构造 `0xE08DC8`、接收 `0xE09D14`、reset `0xE0A288`，以及原 Channel 头、ACK/片段数据边界。部分 resumable/security、偏移、未确认队列行为已经观察到，但没有形成完整可靠性、安全与原运行生命周期实现。

**核对时需注意：**某些后续快捷 xref/ADRP 检索未命中 `0x1743A68` / `0x1743B10`，不能据此否认上述早期证据。原虚表存在 base＋offset 经 GOT 取址，静态 helper 曾漏过这种形式。当前没有闭合其与本项目通话路径的全部绑定关系，所以不能宣称“NetStream 不存在”，也不能把它直接连接到 RTP/Trunk。

## 9. SimAdmin 落地状态与本地安全机制

以下是依据逆向协议实现的 **Linux 侧工程设计**，不把它们冒称为从 APK 原样还原的机制。

| 功能 | 当前状态/边界 |
| --- | --- |
| 独立账号 | `app_account` / `application_data`，私有配置、revision CAS、显式 start/stop；导入不授 readiness |
| 接短信 | 原 `Sip.Request` 文本、durable 落库、历史查询/删除、通知与原回执 |
| 接电话 | 原 CallEx 邀请、应答/拒绝/挂机、history/event、Endpoint/Mpath 与双向 RTP/Trunk |
| 媒体格式 | native raw 或 zlib＋Base64 SDP；API/Trunk raw，支持范围见第 7 节 |
| 正常清理 | 原 controller/actor、consumer、call/media/Trunk Join 完成后释放原运行；SQL 失败不假释放 |
| 自动重连 | 可选、有界；原运行清理完成后同 revision 全新 start；不是续期或旧会话恢复 |
| 退出进程恢复 | 同数据库 OS 锁及 holder 标记允许回收符合该机制的旧运行；不按 PID/年龄猜退出 |
| 外呼/发短信 | 仅保留接口；有效请求仍返回 501，不借 SIM/IMS/CS 兜底或资费 |
| 实网/部署 | 尚未验收；当前软件构建不是设备现状或 Release 发布证明 |

### 运行、重连与恢复的要点

- 运行排他是本地账号/数据库机制，不是服务端 principal/device 全局租约，也不能阻止另一个数据库以同一远端身份登录。
- `cleanup_on_disconnect` 缺省关闭；启用后只清本 manager 的原退休运行。另配 `reconnect` 后，才在原 Join/fence 完成后做有界新登录；鉴权拒绝、过期或版本错误不盲重试。
- 带 holder 标记的进程持有数据库旁 `data.db.app-voice-owner.lock`。新进程取得同一独占锁后，可回收该机制下的旧租约，结束开放应用来电并将发送中的通知记为 `uncertain`；恢复本身不 start。
- 无 holder 的旧版本租约继续显示 `runtime_owner_unconfirmed`；不能按时间、PID 或空内存槽进行接管。
- Voice/Messaging readiness、`hosted`、每通话 `media_active` 和 `outbound_authorized` 分开核验。API `202 start` 只是提交，不能等同于已接通。
- 敏感字段在调试输出中脱敏；本地 `data.db` 仍是明文私有存储，不能声称它是加密保险箱。

完整配置和 API 以 [开发指南](DEVELOPER.md#当前基础托管配置与操作) 为准，主要入口为 `/api/non-3gpp/accounts` 及账号下的 `messages`、`calls`、`notifications`、`trunk`。本文不复制一套可连接服务的示例配置。

## 10. 已取得的验证证据

### 10.1 已核验源码与 Actions

| 项目 | 记录 |
| --- | --- |
| 源码提交 | `2986a3d8b1283d8aaa428d252ab83098a1f311bf` |
| Build-Release | [40585189198](https://github.com/autisticryptic/SimMaster/actions/runs/40585189198) |
| Validate Beta Refactor | [40585189753](https://github.com/autisticryptic/SimMaster/actions/runs/40585189753) |
| 体系结构 | ARM64 / AMD64 构建成功 |
| 发布 | `Publish Release` 按预期 skipped，不是部署或新 Release 验收 |
| 每套核验集合 | 757 Jegotrip、7 registry、17 account HTTP、46 Trunk、31 notification、4 SIP、7 DNS；保留 86 场景矩阵 |
| 该检查点本机检查 | 447 项 Python static/mock 通过；不是本机 Rust 执行 |
| 编译警告 | 每工作流保留 67 条 warning 记录，不声称零 warning |

上述回归集合可能重叠，**不可直接相加当成不重复的测试总数**。核验除了工作流成功，还包括源码提交、逐名测试清单、官方 artifact digest、ZIP CRC 和矩阵结果。Actions artifact 有保留期限，未来不可下载不改变当时本地留存的核验记录。

| 测试制品 | Artifact ID | SHA-256 |
| --- | ---: | --- |
| Build `ims-refresh-tests` | 12772387707 | `2e8e4150867c4b31ffc2e9da720a655d2a1a4efd92a68936a83eaccc238d8477` |
| Beta `beta-refactor-tests` | 12771222636 | `8fa4eda1c46cf697ce3d5f39db44d46dd493bd9f3cb10d10384e5ffca588cafc` |

### 10.2 测试证明了什么

- 已有基础链路测试覆盖生产账号管理器、隔离 TLS/native 协议对端、HTTP 认证、SIP/OperatorLink、实际 loopback UDP 双向 RTP、短信提交/回执、通知及原资源 Join。
- 压缩增量新增 5 项回归：固定 zlib 数据、URL-safe 输入/标准输出、混合/尾随/超限拒绝、解压后 crypto 属性拒绝、实际账号收到压缩邀请后对 API 暴露 raw 并发送压缩 native Accept。
- **最后一项压缩账号夹具的对端故意拒绝 Accept，验证的是编码镜像与拒绝收尾。**它不是压缩通话得到真实服务接受后双向音频的验收；基础双向音频测试和压缩格式测试的证明范围不能偷换。
- 前两次压缩增量提交分别遇到测试 helper 被同名变量遮蔽、旧能力字符串未同步的问题；`9a0e1ca` / `2986a3d` 仅修测试与契约断言。原失败产物保留，没有通过放宽生产策略取得通过。

### 10.3 尚未证明的内容

没有执行原 Android SDK 来与 SimAdmin 做差分互通，也没有连接真实无忧行账号、真实 PBX、真实通知目的地或账单服务。离线测试成功不能证明当前服务器地址、配置、token 有效期、设备限制、并发或收费规则。

## 11. 剩余工作及继续所需证据

| 未闭合问题 | 需要什么 | 当前处理 |
| --- | --- | --- |
| 自动 token 续期 | 自有账号的授权请求/响应、TTL/轮换/撤销错误、设备绑定、活动会话交接 | 手工重新导入；重连不冒充续期 |
| 实际部署参数与长期在线 | 原账号完整参数、服务选择的模式、断线/到期/长时间接收观察 | 不从静态常量或缓存状态猜 readiness |
| 压缩 wire 的完整互通 | 实际邀请/应答样本与原 SDK 上下文，必要时核对预置字典或流状态 | 当前支持有界普通 zlib＋Base64 子集 |
| Locator / NetStream / 全部 Router-Channel | 原调用者、wire schema、路由/缓存/重传与释放状态机 | 局部结构/codec 不扩充为完整 runtime |
| SRTP / ICE / P2P / 转码 | 实际模式选择、密钥/候选/协商与媒体数据证据 | 继续明确不支持 |
| 远端多设备/同账号并发 | 服务端 principal/device/session 规则和行为 | 本地租约不冒充远端排他 |
| 无标记旧 owner | 能证明原持有者不再运行的额外依据或明确人工处置 | 保持 unconfirmed，不自动清除 |
| 接收、Trunk、通知和资费验收 | 另行明确授权的真实环境、有效参数及账单证据 | 不部署、不发起真实服务操作 |

**提供凭据不自动等于授权访问。**后续真实服务、设备或部署操作须单独明确范围；也无需把完整 token、网关密码或私钥粘贴进聊天/提交 Git。

## 12. 代码与证据索引

### 12.1 实现入口

以下模块位于 [`backend/src/connectivity/app_voice/providers/jegotrip/`](../backend/src/connectivity/app_voice/providers/jegotrip/)：

| 主题 | 主要模块 |
| --- | --- |
| 凭据与阶段 | `credentials.rs`、`session.rs`、`controller.rs` |
| Common/RPC | `rpc_stream.rs`、`rpc_envelope.rs`、`rpc_login.rs`、`rpc_batch.rs`、`rpc_arguments.rs`、`rpc_logout.rs` |
| 下层边界与 TLS | `packet_stream.rs`、`router_packets.rs`、`tls_plaintext.rs`、`tls_engine.rs`、`transport_actor.rs` |
| 配置/引导与连接 | `legacy_config.rs`、`object_reference.rs`、`deployment.rs`、`runtime_provision.rs`、`runtime_connector.rs`、`runtime_backend.rs` |
| 消息/持久化/回执 | `rpc_incoming.rs`、`smgw_text.rs`、`account_inbox.rs`、`account_sms.rs`、`account_notifications.rs` |
| 长期接收 | `direct_history.rs`、`batch_scheduler.rs`、`batch_retention.rs`、`account_runs.rs` |
| 原来电控制 | `rpc_calls.rs`、`call_binding.rs`、`call_control.rs`、`account_calls.rs`、`account_call_history.rs` |
| SDP/媒体 | `native_audio_sdp.rs`、`rpc_endpoint.rs`、`media_endpoint.rs`、`media_endpoint_runtime.rs`、`tcp_mpath.rs`、`tcp_mpath_audio.rs` |
| 原账号运行与恢复 | `account_config.rs`、`account_store.rs`、`account_runtime.rs` |
| 局部 Router/Channel codec | `native_router.rs`、`native_channel.rs`；不代表完整 NetStream runtime |

### 12.2 关键 native 定位速查

| 主题 | 该样本中的地址 |
| --- | --- |
| bootstrap / 登录 | `0x58C98C`、`0x58F6F8`、`0x580BA4` |
| Common 方法版本列表 | `0xD747CC`、`0xD69408` |
| PacketSender 长度边界 | `0xDFF2A8`、`0xDF0DC4`、`0xE712DC` |
| Router 头/接收 | `0xD61F4C`、`0xD61A38`、`0xD68010` |
| batch 字段/ACK | `0x4A3914`、`0x4A6774`、`0x590D98`、`0x594974` |
| DIRECT magic 窗口 | `0xDC2300`；不要把 `0xD9F118` 构造函数误当窗口逻辑 |
| 精确 Sip.Request 注册与通知 | `0x510E3C`、`0x524104`、`0x524110`、`0x523EC0` |
| CallEx 通知 / Accept / BE64 | `0x484788`、`0x479DE8`、`0x479C2C`、`0x47963C`、`0x477074`、`0x4757E4`、`0xE70B6C` |
| SDP 检测/读写 | `0x613140`、`0x5FA164`、`0x251958`、`0x60ED60`、`0x5FA47C` |
| Mpath 头/发送/接收 | `0x659D60`、`0x659D94`、`0x664188`、`0x664620` |
| PBE 派生/编码/解码 | `0xF56D24`、`0xF56E08`、`0xF57050` |
| Locator 名称注册 | `0xDBB6A0` 附近、`0xE816AC`；不是完整 locator wire |
| NetStream / Channel 局部结构 | `0xE08DC8`、`0xE09D14`、`0xE0A288`、`0xF509F0`、`0xF53028` |

### 12.3 本地证据路径

以下文件属于忽略的私有分析目录，公共 Git checkout 中可能不存在；这里只列位置，不搬运原始反编译全文或秘密值：

```text
.local/jegotrip/resume-20261027/original-apk-identity.json
.local/jegotrip/verified-dex-call-paths.json
.local/jegotrip/deployment-offline-evidence.md
.local/jegotrip/bootstrap-handoff-implementation-evidence.md
.local/jegotrip/resume-network-framing-evidence.md
.local/jegotrip/tls-offline-evidence.md
.local/jegotrip/batch-inbound-offline-evidence.md
.local/jegotrip/smgw-text-classification-evidence.md
.local/jegotrip/runtime-exclusion-call-evidence.md
.local/jegotrip/tcp-mpath-audio-implementation-evidence.md
.local/jegotrip/native-router-channel-evidence.md
.local/jegotrip/static-20261028/apk-renewal-evidence.md
.local/jegotrip/static-20261028/dex-phone-token-all-xrefs.json
.local/jegotrip/static-20261028/dex-refresh-xrefs.json
.local/jegotrip/static-20261028/native-mode-selection-evidence.md
.local/jegotrip/static-20261028/native-sdp-callrefs.txt
.local/jegotrip/static-20261028/compressed-sdp-verification-summary.json
.local/jegotrip/static-20261028/compressed-sdp-first-failure.md
.local/jegotrip/verified-native-2986a3d.json
```

旧证据文件中的“尚未实现”只属于其记录的提交阶段；查当前功能时应结合源码基线和 [HANDOFF](HANDOFF.md)。反过来，后续摘要中的简化表述也不能抹去更早的正向字节证据，例如 NetStream 的 GOT/base＋offset 构造定位。

相关手册：[开发指南](DEVELOPER.md) · [开发计划](DEVELOPMENT_PLAN.md) · [协议与权限边界](IMS_REGISTRATION_POLICY.md) · [更新记录](CHANGELOG.md) · [文档导航](README.md)
