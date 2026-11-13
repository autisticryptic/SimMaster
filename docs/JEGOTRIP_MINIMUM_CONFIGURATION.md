# 无忧行托管：最小配置与授信材料

> 2026-11-04 对原 APK、当前 Rust 实现的交叉核对。这里的“最小”指普通用户需要输入什么，**不是删除尚未确认的接收器/媒体参数，也不是完整托管已经验收**。

## 1. 直接结论

- **不需要用手机硬件或模拟器生成 App UUID。** 原 App 读取 `phone_uuid` / `device.dat`；没有值时生成随机 UUID 并持久保存。名字为 `getIMEI()` 的这条函数返回的不是物理 IMEI。
- 常规登录只输入**已绑定号码的本人账号、密码**，并明确选择是否开启托管；手机号密码模式另需区号。App UUID 默认由 SimAdmin 生成、按认证号码持久复用，手工指定是可选项。
- **没有证据支持“导入一串长期授信代码，替代所有设备及连接配置”。** UUID 是标识；运营商认证 token、App 会话、电话 token、SIP 密码用途不同。服务器是否认可该账号/号码/设备组合才是授权依据。
- 服务地址、证书、SDK 应用参数是**安装侧共享协议上下文**，不是每个账号都要填写的手机硬件。普通用户不用反复编辑；目前由私有高级安装配置提供，尚未全部变为内置默认值。
- SDK 设备标识和 App UUID 是两条数据流，**不能未经核验统一替换**。现已追通正常 Android SDK getter：JNI 调用 `ZpandDevice.getUuid`，读取独立持久随机 UUID；`ClientId` 的默认前缀来自包名，再拼接该 SDK UUID，不由物理 IMEI/型号计算。原生后备/显式覆盖与服务端风控仍须分别处理。

### 授信是什么、UUID 是否必要？

**可观察的授信是服务器对当前账号/绑定号码及 App 设备标识的认可状态，不是一张保存在手机上的、可复制到任意设备的证书。**
App 只有收到外层 code0、`authentication_code=1` 后才进入成功分支；收到 setVoip506 会删除本地号码缓存。

- 原 App 的 enroll / securityVerify 明确发送 `deviceId`；已有设备查询与 setVoip 明确发送 `device_id`。因此 **UUID 是原 App 授信/业务通路必须保持一致的材料**，不能因为手机认证过就省掉或每次重新生成。
- 这证明它是原请求的必要内容，**不证明服务器对所有接口/账号只检查 UUID**。是否还校验其他上下文、删掉字段会如何拒绝，服务器实现不可见；本轮不通过猜测请求测试它。
- 已确认 App UUID 及正常 Android SDK UUID 的生成均不读物理硬件来计算。首次认证可能使用 SIM/运营商能力，这是验证号码的来源，**不能据此说 UUID 是硬件指纹**。
- 是否能仅复制本人 App UUID 就复用手机授信，需要在手机真实授信后，用同一账号/号码在 SimAdmin 查询服务器状态并实测。**本轮不把静态结论当成已经验证可迁移。**

### 手机 → SimAdmin 的配置通路

1. 在本人手机中完成合法设备授信；保持 App 数据，不为取 UUID 清缓存或重装。
2. 取得原 App `setting_power / phone_uuid` 或 `device.dat` 中的原样 UUID，保留大小写与连字符。这两处为 App 私有存储，本文不声称非 root 手机可直接读取；若 App 未提供导出入口，需要用户自己的合法读取方式，不自行安装工具或修改手机数据。
3. 首次配置可先从原始 `setting_power.xml`、可读 `device.dat` 或离线脚本的 UUID-only JSON **在浏览器本地读取**；普通 root ZIP 先在本地解析，SBA1/v1::: 封装需原工具导出解密数据。读取不保存、不登录、不勾选授权，也不导入 token/缓存授信。详见 [手机备份读取说明](JEGOTRIP_PHONE_BACKUP_IMPORT.md)。随后在登录页选择 **“复用手机 App 已授信 UUID”**；已有账号先在 **“设备标识管理”** 选择原号码，再点 **“录入手机 App 授信 UUID”** 做有版本守卫的替换，旧值所有权保留。36位 UUID 专用输入会拒绝误粘的 MD5、enrollId、IMEI 等。选择手机 UUID 后默认采用 `reuse_hosting`，不是普通自动生成/enroll 登录。
4. 可先选择 **“只查询设备授信”** 做独立诊断：正常账号登录、确认号码后，查询 securityVerify、已有设备后备和业务状态；**不 enroll、不取电话凭据、不建原生账号、不改业务、不启动接收器**。会保留 App 认证摘要和此次观察，不保留密码/App 会话；该旧诊断历史不授予后续动作。
5. 正式复用选择 **“复用已授信 UUID 接管托管”**（`reuse_hosting`）：保持明确指定的原 UUID，不 enroll、不请求 VTD、不携静态旧 enrollId，也不会因发现同一号码已有另一 UUID 而静默切换。以本次 `getCallStatus.authentication_code=1` 为门禁，`cs_forward=2` 才接管；未明确认可或业务未开立即停止。
6. 已授信但业务未开时，另选 **“复用已授信 UUID 并开启托管”**（`enable_trusted_hosting`）并明确同意业务操作。取得电话凭据/校验原生模板后、写开关或建账号前会再次读回；开启后同时确认认可码和业务。未知或拒绝不自动重放，不申请新设备授信。
7. `device/v1/sync` 是原成功分支中的独立设备报告：仅安装侧明确配置本人手机真实资料时发送；`device_id/imei` 固定为该原 UUID，`userId` 取本次 user-info 的原 `accountid` 字段。不猜硬件/push token，未配置不发送。报告接受和 `localCsForward` 均不代表 SDK/媒体就绪。

用户提供“同一时间一个设备授信”的规则，当前将其作为安全边界，而非已独立实测的服务端表结构。复用不取消手机授信、不为另一 UUID 发 VTD；手机同时在线还可能触发**独立的 SDK 会话冲突**。后续仍需原生收件/停止清理验收，不能把模拟成功当作真实迁移。

**应迁移的当前最小候选是本人已授信的 App UUID + 同一本人账号/号码上下文。**
不复制 `phone_safe_num` 的 MD5、`authentication_code` 缓存、旧 enrollId、App 会话或运营商 token 来冒充授权；
不把 SDK 的另一个 UUID 误当成 App 的授信 UUID。是否还需要其他服务材料，按真实查询结果继续核验。

## 2. 各种“一串代码”不能混用

| 材料 | 原 App 来源及用途 | SimAdmin 的处理 / 边界 |
| --- | --- | --- |
| App `deviceId` / `device_id` | `DeviceUtil.getDeviceId → getIMEI`，持久随机 UUID；用于 enroll、安全设备查询及业务开关 | `jegotrip_device_identities` 按认证号码保存；随机或手工本人值。不是物理 IMEI，不创造授权 |
| 本地 `phone_safe_num` | 区号+号码的 MD5 缓存；仅服务器验证成功后写入，setVoip506 后删除 | 不能把此标记作为服务器授信或当前接收就绪 |
| `authentication_code` | 服务器返回的状态，App 另有本地状态缓存 | 状态不是可导入的认证凭据；HTTP200、code0 或本地布尔值都不能单独替代业务读回 |
| `enrollId` / `expireTime` | 匿名设备注册的服务端结果；登录请求携带 enrollId | 普通登录兼容路径自动取得；明确授信复用路径不调用 enroll、不携静态旧值。登记成功不是授信，不宣称永久有效 |
| `sso_token` | `MobileAuthUtil` 的真实运营商认证回调第2参数 | 不是 App 登录 token。旧诊断可按 receiveFail 后备传空字符串；明确授信复用用 getCallStatus 原认可分支，不进入 VTD 或新运营商认证 |
| App 会话 token | 正常账号登录响应，用于后续 App HTTPS 身份认证 | 仅当次流程使用，不作为 sso_token、phone token 或设备授权 |
| 电话 token | 原 App 的 `csb/token` / `getPhoneToken` / `CallUserInfo.cstToken`，传给 `JCClient.login` 的 password 参数 | 正常登录流程自动取得；不是用户密码、运营商 token 或 SipPwd |
| SIP 密码 | `openVoipStatus` 返回的 SIP 材料，原 App 解密后用于网关 | 后台取得并解密，不要求用户手提；不与电话 token 混用 |
| ARC 返回 token | 原生 `loginSession` 成功回复 | 只属于匹配原生账号/连接，不是 App 会话或设备授信 |
| SDK `DeviceId` / `ClientId` | SDK 的独立配置和原生生成/上报链 | 正常 Android getter 已追到独立的 ZpandDevice.getUuid；ClientId 默认由包名+SDK UUID构成。与 App UUID 分开，不由手机硬件推算，也不能随意统一 |
| 厂商推送 token | `RichPushManager` / `JCNewPush`，真实平台注册材料 | 与在线长连接、设备授信无关；Linux 版未实现 Android 厂商离线唤醒，不能填写假 token |

**同 UUID 复用**只是在同一账号/号码上下文中保持身份一致，不是复制一份授权到其他号码。
授信期限、撤销条件、账号年龄策略以及是否还检查其他字段，服务器内部规则仍未知。

## 3. 普通用户与安装维护者的配置分层

### 普通用户

1. 已绑定号码的本人账号和密码；手机号登录按实际入口填写区号。
2. 明确选择“开启托管”或“接管已开启业务”。接管不写服务端开关，但仍需业务状态允许。
3. 默认自动生成/复用 App UUID；需要保持自己已有身份时，可手工指定自己的值。
4. 服务端确实要求安全验证时，完成它认可的本人号码/设备认证。不能由程序编造挑战或 token。

**不要求普通用户输入硬件 IMEI、手机型号、制造商、Android ID、MAC 地址、分辨率，也不要求手填电话 token/SIP 密码。**
这说明当前登录输入和 UUID 生成不需要这些字段，**不等于证明服务器任何风控或原生 SDK 都不检查终端信息**。

### 安装侧字段

| 当前配置 | 类型 / 来源 | 是否可以省略或自动取得 |
| --- | --- | --- |
| `version` | 安装配置 schema，当前为1，不是无忧行 App 版本 | 保留 |
| `http.base_url`、`endpoints` | 原 App 服务地址和端点目录 | 一次安装上下文；目前不是用户账号级输入，不随意猜测/删除 |
| `anonymous_token`、签名/口令 salt、SIP 解密 key、`body_protection` | 原 App 共享协议参数 | 私有安装上下文；不能被 UUID 或授信状态替代，不在本文提供秘密值 |
| HTTP 证书固定及公钥 / 根证书 | 显式可信服务连接 | 保留严格校验，不用减少配置作为跳过 TLS 的理由 |
| `http.device_identity_mode` | 本地身份策略，默认 `store` | 可省略，默认按认证号码持久管理 |
| `http.device_id` | 旧固定值兼容 | `store` 新安装可省略；已有值不擅自旋转；`static` 模式必须给值且不可跨号码复用 |
| App 版本来源、共享状态表、`app-version` 请求头 | iOS 商店/官方页面/手工值；首次拉取，524 后有界刷新 | 已动态化，无需每次官方更新都重编译；query/body 不是已测成功的承载 |
| `user_agent`、`enroll_id` | 可选安装兼容项 | 普通流程自动 enroll；明确授信复用不携旧 enrollId、不自动登记 |
| `http.endpoints.device_sync` / `http.device_sync_profile` | 可选原 Android 设备报告上下文 | 仅明确授信复用、本次认可码1后可发送；版本/版本码、device_type、os_version、channel_id必须来自本人手机真实观察，可选push/MAC/品牌/型号/分辨率未知则省略；不允许profile配置设备或userId目标 |
| `runtime.sdk.application_key`、逻辑入口、信任及独立解码 key | 原 SDK 共享上下文 | 安装侧保留；应用 key、Router access、Client 解码 key 和 reference key 不能互相替换 |
| `runtime.sdk.device_id` | SDK 的可选显式标识 | 未配置时并非 App UUID 自动替代；不要求复制手机硬件信息来“生成”它 |
| `runtime.sdk.selected_session_id` | 原生会话选择值，原默认 `<UnifiedSession>` | 是 selector，不是 token。当前显式原生模板仍要求正确值；本轮没有偷偷新增默认值 |
| `runtime.hosting` | 网关 AuthnamePrefix / ChannelId / DeleteIntCode | 安装侧协议参数，不是授信代码 |
| `profile.route.router_common` | 物理 Router、`-ri`、对应可信 anchors、固定预算 | 保留原地址/身份/证书对应关系，不能删 `-ri` 或把备用提示当 TLS 授权 |
| `profile.bootstrap_via_router` | 真实 `getAppConfigs` 后交接 | 目的地及 DomainName/DomainId/AppId/CC 来自匹配回复；不能预填猜测值 |
| `profile.gateway_uri` / `gateway_metadata` | 匹配 loginSession 身份 / 原默认 `#SipGateway` | 正确 native bootstrap 模式下空 URI 动态派生；其他路径仍要求实际值 |
| `incoming` / `call_incoming` / `call_control` | 原对象、域及通话路由策略 | **仍有装配缺口**。发现对象字符串不等于完整绑定，不能删检查让页面显示就绪 |
| `call_media` | 原媒体可信连接与本地音频/Trunk 策略 | 需正确配置与真实双向媒体验收，不是手机硬件身份 |
| `cleanup_on_disconnect` / `reconnect` | 本地有限生命周期策略 | 可选，默认行为保留；不是 token 续期或 Android 离线唤醒 |

完整 JSON 仍是高级安装入口；**本表不是一份可直接导入的完整部署模板**。
数据库不回显秘密，不是加密保险箱；配置、数据库、APK 和私有证据不提交到公开仓库。

## 4. “可以短信授信吗”的新增核验

原 SDK 存在 `AuthnHelper.sendSMS` / `getTokenSms`，但本次正确解码8个内嵌 DEX、扫描
**263527个 Java code item**后，未发现对这两个入口的 Java 调用；当前 App 封装调用的是
`getTokenImp`，有两个 Android 权限/版本分支调用点。

因此只证明**SDK 有能力入口，不证明这份 App、当前账号、当前服务端允许用它解除506**。
原生代码、反射调用或服务器其他策略不在“没有 Java xref”的排除范围，不能写成绝无替代方案。
这也不同于登录返回88001、authType4的风控短信挑战。

运营商 SDK 还保存自己的缓存记录，并检查到期时间、上下文匹配，失败时清缓存；相关分支有
`currentTimeMillis` 比较和 `clearCache`。这类缓存不是一串永久设备授权，不能从“有缓存”推出
可以无限重放认证 token。没有核实其业务有效期之前，不把缓存结构移植为免验证机制。

## 5. 原字节证据与本轮范围

原 APK：`com.cmi.jegotrip`，manifest `8.9.7.0` / versionCode8970。
APK SHA256：`9874f23c3cfde41cecfc23b67b6da7d81e161a81cc875d05fc33483b89478a1e`。
libmtc SHA256：`57086cd624d8227dc1b1b64a63d7c8b2258f87b261779b681a9bfd306ad9d2e8`。
8个 DEX 均核验 SHA1、Adler32 和表/指令边界；PC 为 code unit，code_off 为 DEX-relative。

| DEX base / code_off | 证据 |
| --- | --- |
| `0x8e0000 / 0x80eab0, 0x80ebd8` | App `DeviceUtil.getDeviceId/getIMEI`，本地稳定 UUID 路径 |
| `0x8e0000 / 0x634c58` | receiveToken 的第2参数送 querySafeDevice；失败分支 `0x634c00` 送空字符串 |
| `0x8e0000 / 0x85df28` | querySafeDevice 的三个正文键与普通 HttpRequest.request |
| `0x8e0000 / 0x6f26d8, 0x6f9eb8, 0x6fa910` | enroll、安全查询、独立终端报告三条链，不是同一份请求 |
| `0x8e0000 / 0x861d14, 0x861e44` | 运营商结果 token → App 回调；当前使用 getTokenImp |
| `0x8e0000 / 0x870ca0, 0x870d24` | SDK 短信入口存在，Java xref扫描为0 |
| `0x8e0000 / 0x877218, 0x87759c, 0x8777f0` | 运营商缓存到期/匹配/清除分支；不证明永久 token |
| `0x1aa0000 / 0x5ca7a4, 0x5cab94, 0x5d5ac8` | SDK deviceId 的独立配置/getter；最终 getter 进入原生 Mtc_CliGetDevId |
| `0x1aa0000 / 0x5d6080` | SDK 登录另外报告 OS/型号/制造商，不能只看 App UUID 就排除这些信息 |
| `0x1aa0000 / 0x5ea31c, 0x5ea38c` | ZpandDevice 的两个独立持久随机标识；IDA 已核实 native getter 走 getUuid，不是 getId 或 App phone_uuid |

本地原始脱敏证据：`.local/jegotrip/minimum-config-audit-20261104/` 的 provenance及方法dump，
`.local/jegotrip/resume-20261104/minimum-input-xrefs.json` 的全部调用索引。

本轮只读 IDA 新核验：`Mtc_CliGetDevId 0x6acffc → 0x6a51c0 → 0x24f17c → 0x242124 → 0x62173c`；
`0x4e39dc` 把 JNI slot `0x178e880` 绑定到 **ZpandDevice.getUuid**；正常 getter 因此读取独立持久随机 UUID。
`0x584218` 写 SDK 配置设备字段 `+5816`，`0x657fe4` 发送该 DeviceId，并拼接 `+5560` ClientId 前缀；
该前缀从 `0x24f174 → 0x24efa4 → Context.getPackageName` 得到。后备 `0x4e89f4` 为另外的 native UUID 路径，不把未知分支说成 App UUID。

当前实现对应 `app_login/{config,http,device_identity,device_trust,device_sync,trusted_reuse,mod}.rs`、`account_config.rs`、
`runtime_backend.rs`、`runtime_provision.rs`；生产后端仍为 Rust，Python 仅静态分析辅助。

### 本轮已授信复用的新静态定位与验证状态

- `0x8e0000 / 0x85da9c`：queryCallStatus明确携原device_id的普通JSON。
- `0x8e0000 / 0x61a0e0`：从body读取authentication_code、cs_forward、**localCsForward**；不能猜蛇形字段。
- `0x8e0000 / 0x633a44, 0x636078`：getCallStatus认可码1直接走成功+updateDeviceId，不先重新取token/写VTD。
- `0x8e0000 / 0x5f3704, 0x67be40`：sync.userId来自User.accountid；原Gson SerializedName已核为accountid。
- `0x8e0000 / 0x6f5b3c`：updateDeviceId完成回调仅记录状态，不是业务/SDK grant。

新增源码与模拟用例必须由**自身SHA**的两个Actions和官方制品验证；未通过前不是已交付绿色版，不能借旧6f85149的1249回归冒称成功。真机仍需本人已授信UUID与真实读回条件。
脱敏新证据 `.local/jegotrip/trust-reuse-audit-20261104/`；模拟不证明服务器的单设备/撤销/期限规则。

本轮未执行 APK/SDK、流量认证、验证码、业务开关或真实短信/电话。
初次 IDA MCP 连接被拒绝后，用户手动打开原 libmtc；实时 metadata 的 SHA/base 已匹配。
后续全部仅只读查询，不保存或修改 IDB；这些字节证据仍不是手机授信迁移成功的实测。

### 原生本地接收器配置（新增，须自身SHA验证）

原生`Router/Common`模式可显式配置 `runtime.profile.native_receivers: {}`，使用已核对的本地`MessageReceiver/Sip.Request`与`CallClientEx`对象；与旧`incoming`、`call_incoming`互斥，缺省仍保留旧精确绑定，不改已保存模板。新模式不从出站`domain`、App UUID或DomainName伪造接收域。

`native_receivers.direct_domains`可省略（原空deny-list），或明确给 `{ "mode": "allow_list" / "deny_list", "values": [字节数组...] }`。它仅过滤同一原生连接的DIRECT请求；已匹配外层账号/epoch/token的batch走独立本地对象表，不重复DIRECT域检查。两个路径仍严格校验原owner/scope、对象、方法、计数及大小。当前只支持无facet/adapter/cloud限定的精确对象；不做默认定位器或任意引用回退。

`client_ex`、`offline_reception`仍是独立明确选项；来电控制`call_control`及媒体配置不被推断，独立Alive未注册时checkAlive仍false/0。早到短信仅在原HostingPending预算内有界等待，不在确认前入库或ACK；后续原包/SMS/通知事务、删除、cursor和Join规则保持。native inbox的单一接收domain记NULL，表示无此固定绑定，不冒称记录了一个远端域，也不回填旧记录。

本轮新增生产代码及回归需自己的双Actions与正式制品验证，当前最新状态见[交接](HANDOFF.md)。

## 6. 仍需继续完成的部分

1. 旧任务36：真实接收器默认/动态绑定、适用 AccountAlive、来电通知及媒体配置仍需完成。原生 `moreOfflineMessages → getOfflineMessages` 积压软件已由0c4的自身双Actions/正式制品核验，详见[原生离线积压](JEGOTRIP_NATIVE_OFFLINE_INBOX.md)，不等于真实收件或完整SDK。新增 `profile.client_ex` 默认为false，仅复用已有原绑定；空通知map唤醒、原token匹配撤销及不可重连错误已由38ae8d7自身双Actions/正式制品核验，不等于默认/动态SDK全部装配。它不自动填接收域、不申请Alive；无独立注册证明时checkAlive仍false/0，详见[原生控制子集](JEGOTRIP_NATIVE_HOSTING_DELIVERY.md#显式-clientex-控制子集)。
2. 旧任务16：合法业务条件下真实原生登录/注册、短信落库与原ACK、来电/双向音频和 stop/Join。
3. 旧任务17：完整验收后最终收尾。**最小配置分析、界面简化或已有1242项隔离回归都不能代替这三项。**

参见 [交付与分层诊断说明](JEGOTRIP_NATIVE_HOSTING_DELIVERY.md)。
