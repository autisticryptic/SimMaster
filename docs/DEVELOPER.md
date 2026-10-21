# 开发、验证与交付

本页是代码工作流入口；架构见[ARCHITECTURE](ARCHITECTURE.md)，安装见[INSTALL](INSTALL.md)，
当前未完成事项只维护在[HANDOFF](HANDOFF.md)和[DEVELOPMENT_PLAN](DEVELOPMENT_PLAN.md)。

## 1. 分支与编译约束

- 无忧行模块：本机只解析 APK/嵌套 DEX/ELF 和运行静态/mock；APK、提取库、账号 token、网关密码、SDK app key/部署值只留 `.local`，不提交。Rust `connectivity::app_voice::providers::jegotrip::{tests,controller::tests}` 由两套 Actions 执行；元数据 API 只描述凭据，不连服务。详见 [静态勘察](DEVELOPER.md#无忧行托管静态证据凭据与实现边界)。

- 日常仅维护`master`，GitHub远端为`autisticryptic/SimMaster`（本地remote通常为`simmaster`）。
- 不为每次验证长期留下分支。确需临时分支时，合并并确认提交已保留、CI通过后删除。
- 不force-push主分支，不清理未知用户工作，不提交私有SIM材料、日志或凭据。
- **Rust/前端构建、编译后的测试及注册模拟只在GitHub Actions运行，不在本机编译。**
- 本机可以读/编辑代码、运行不构建的Python/静态检查、查看Git差异、下载并验证产物。
- 不把本机旧二进制、旧HEAD的测试报告冒充当前工作树通过；报告绑定完整commit和源码摘要。
- 运营商无关的兜底修复用协议/状态机测试，不引入MCC/MNC专属代码或专属测试掩盖全局问题。

## 2. 项目布局

| 路径 | 职责 |
|---|---|
| `backend/src/api/` | HTTP、认证、请求与响应模型 |
| `backend/src/connectivity/core/` | 接入无关SIP/AKA、注册及业务模型 |
| `backend/src/connectivity/modems/ims/` | 蜂窝IMS、VoWiFi、profile适配 |
| `backend/src/hardware/` | 协议后端、SIM与设备驱动 |
| `backend/src/services/` | 线路/worker、业务路由、Trunk、自动化、通知 |
| `backend/src/platform/` | 配置、SQLite、DNS、命名空间和系统工具 |
| `frontend/src/api/` | API契约和封装 |
| `frontend/src/pages/`、`components/`、`hooks/` | React工作台与组件 |
| `bruno-api/` | API调试集合 |
| `.github/workflows/` | 构建、回归及发布门禁 |
| `scripts/`、`deploy/` | 统一打包、安装器和驱动资源 |
| `offline-registration-sim/` | 无设备REGISTER协议模拟，不能当实网证据 |
| `.local/` | 私有诊断/备份/历史材料，不提交 |

## 3. 修改边界

新接口/能力应同时核对：后端DTO和认证、路由、前端类型/封装、实际页面、Bruno请求、兼容编码和测试。
所有线路操作显式解析line_id；禁止重引入“第一台modem”隐式全局控制。
设备能力按driver声明，不从名称猜QMI/AT能力，不在通用层硬编码某个QCM410端点。
同一物理端口只有一个owner；串行锁不代替SIM、MM owner、代次及资源归属验证。

展示投影与可操作runtime分离：恢复门禁可以阻止写入和承载，但不能把原线路页面替换或清空。
保留原工作台的列表/详情/标签/保存意图，仅在必要位置提示未知/恢复中，并禁止对应危险操作。

IMS 执行诊断用 `core/registration_diagnostics.rs` 的白名单类型，不回显身份/密钥/原 SIP。
在实际 provider 请求、UICC 选择、通道建立、报价发送、验证后提交及真实门禁处观察；不得由 profile 标签估计结果。
epoch/serial 和弱 runtime 只限制诊断写入，不替代原归属/预算/资费校验。最近成功不是当前 readiness，响应 Retry-After 不是已执行的等待。

## 4. 配置与API

配置文本和SQLite分层由`platform/config_file`、`config_store`及`ConfigManager`负责。
YAML写后重解析，保留未变注释；线路映射/业务历史不因重构批量改写。
新增字段必须有默认/迁移规则，拒绝重复别名及不支持的顶层键。

除明确公共的健康和认证入口外，业务API默认受会话保护。不要为了调试绕过认证、重置账号或回显密码。
每个请求确认方法是否实际有副作用：GET命名不自动保证不会启动探测或清理旧对象。
契约细节见[IMS协议](IMS_REGISTRATION_POLICY.md)和[原生后端](NATIVE_BACKEND_STATUS.md)。

## 5. Actions验证

主要工作流：

- `Validate Beta Refactor`：硬件无关Rust、配置/兼容边界、私有D-Bus/HTTP回归及前端检查。
- `Build-Release`：前端、后端测试、ARM64/AMD64构建、同版本完整包和发布门禁。
- `Frontend Checks`：路径匹配时运行lint/类型/构建与隔离Playwright工作台回归。

新Rust测试必须加入执行过滤器；仅`cargo test --no-run`通过不表示执行过测试。
N8 `22646c8` 三工作流/双架构、各417项后端相关回归/86矩阵场景、46单测/4原工作台browser及官方ZIP/源码/截图已核验。新增 `hardware::devices::ims_bearer_policy::tests`、`transport::capabilities::tests`、`backends::wds_pcscf::tests`/`io::tests::retained_wds_`，双接入profile/record/override与真实provider/family/worker/DHCP回归；私有D-Bus执行`http_router_tests::ims_bearer_`。首轮browser断言误命中说明文字已修复，后续IPv6部分schema补强也重新验证，失败证据保留。承载能力是driver声明，不是网络授予/readiness；PCO reporting不等于request-policy。存储保留显式凭据，Debug/API只暴露方法与存在标志，旧客户端遗漏不得清空。
过滤器选中0条应失败；下载日志逐名核验新增回归，避免只看绿色workflow。
X2 `ca38391`新增 `backends::native_ims::tests`、`quectel::native_ims::{tests,serial_tests}`、`services::native_ims_observation::tests`、core排他/双接入idle及私有D-Bus的`http_router_tests::native_ims_observation_api_`已实际执行；三工作流/双架构、各462项后端相关回归/86场景、49单测/12browser和官方ZIP/19源码/三张原工作台截图已核验。首轮3处fixture非空断言lint失败已修复并保存，不连接设备。
ISIM/端点/恢复第一批另包含 `qmi_uim::isim::tests`、`isim_binding_tests`、`uicc_ims::tests`、
`vowifi::live::live_identity::tests`、`pcsc::ims::tests`、`cellular_ims::pcscf::endpoint::tests`、
`register_retry::tests` 和 `live::live_retry::tests`；实际刷新和 profile batch 的新增测试纳入原过滤器。
REGISTER rspauth 另执行 `core::digest_aka::response_auth::tests`、`vowifi::live::response_auth_tests`；R3首次认证后423另以非零过滤器执行其`initial_min_expires_tests`，必须核验实际VowifiRegisterAuthenticator/同通道/原预算和新proof，不用仅helper或旧CI代替。
蜂窝测试位于原 `cellular_ims::live::refresh_tests` 过滤器下的 `response_auth_tests`；继续执行共享 REGISTER 及既有 AUTS/stale 回归。
reg-event 执行 `core::reg_event::tests`、`cellular_ims::live::live_reg_event::tests`、
`vowifi::operator::reg_event_tests`、`vowifi::live::reg_event_live::tests`；DNS TCP 另执行
`cellular_ims::pcscf::dns_transport::tests`，NAPTR 位于原 DNS/endpoint 测试过滤器中。
TCP/TLS 另执行 `cellular_ims::channel::sip_stream`、`stream_channel::tests`、IPsec 及原刷新过滤器内的 `stream_registration_tests`。
N7 另执行 `core::registration_diagnostics::tests`、`cellular_ims::runtime::diagnostics_tests`、
`vowifi::live::diagnostics_live::tests`，实际 cellular/WiFi 刷新观察位于现有 response-auth/refresh 过滤器；
原生承载 observer 测试在 `native_bearer::tests`，统一状态 HTTP 契约在私有 D-Bus 中运行。
刷新预算补漏的 `refresh_budget_tests` 与 TCP EOF/423 回归位于既有 `cellular_ims::live::refresh_tests` 执行过滤器；新增测试仍须逐名核验，不用静态 Python 守卫替代生产交换。R1 `b6a7888` 的两套日志各257项相关回归/四矩阵和双架构已核验，前端未变沿用并复核 N7 browser 源码证据，不冒称每次后端提交都重跑浏览器。
R9 执行 `vowifi::qmi_uim::refresh::tests`：刷新帧/TLV/事件解析、session/AID 过滤、忙时投票与 20 秒上界、超时不当作致命、非法输入不订阅；仅纯逻辑与 QMUX 往返，不连 modem。
R7 `d373ffa`两工作流/双架构、各522相关回归/86场景已核验；执行 `cellular_ims::ipsec::worker_tests`：原binding入队/回包代次、已过期binding零命令、FIFO生命周期锁窗口、旧响应不跨代回滚，以及同代部分失败原错误保留；测试仅使用真实worker消息协议的内存响应器，不执行内核XFRM。
R6 `6672a64`两工作流/双架构、各517相关回归/86场景已核验；执行 `cellular_ims::live::pcscf_refresh::tests`：真实provider/worker/REGISTER、无PDN重激活、旧保护继承、临时等待、partial/source/worker负例；共享lease测试验证响应后的复核不能重置租期时钟。
R5另执行 `uicc_ims::provisioning::tests`、`cellular_ims::live::refresh_tests::provisioning_tests`、`vowifi::live::response_auth_tests::provisioning_tests`；验证同卡变更/撕裂读取/读失败、实际REGISTER提交门禁、旧nonce/租期不升级及已收到等待保留，不将静态检查视为硬件事件支持。
R4注销反馈（0351006已通过两套Actions/双架构、各488相关回归/86场景）另执行 `core::register_retry::unregister_tests`、`cellular_ims::live::refresh_tests::unregister_retry_tests`、`vowifi::live::response_auth_tests::retry_unregister_tests`；检查真实注销/清理/后继门禁与旧卡回包隔离，diagnostics hook不能写控制状态，静态守卫不代替Actions。
R2 另执行 `vowifi::live::register_retry_live::tests`、`vowifi::runtime::retry_tests`，`response_auth_tests::retry_recovery_tests` 覆盖真实刷新及安装后的operator收包/到期，私有 D-Bus 另测 API 等待不改保存意图或重耗预算。核心等待下沉后保留 cellular 的旧测试过滤器名称和全部边界断言。R2最终 `c3fb036` 的两套日志各304项相关回归/四矩阵/官方摘要已核验，另逐名确认原认证前404回归未改且通过；不沿用中间 e06b0ed 结果代替新增撤销竞态保护。
Frontend Checks 必须实际运行 `imsDiagnosticsFormat.test.ts` 和 `ims-execution-diagnostics.spec.ts`；
后者使用原工作台组件、mock API、关闭设备代理，检查来源/等待/旧响应/跨线路及零配置写入，不调用实机端点。
N7 已验证代码 `ee9ee28`：两套后端日志各250项相关回归，前端43单测/3browser、官方制品摘要、source/run和原界面截图已核验；新代码需重新执行，不能沿用此结果。
GitHub artifact 下载需要认证时，用户可直接提供未解压 ZIP；先对比官方 artifact SHA-256，再只读检查日志/JSON，不要求用户提供 token。
私有D-Bus和fake peer必须与真实系统总线、modem和公网隔离。

注册模拟在Actions runner执行，例如：

```bash
python3 -B offline-registration-sim/run.py --report offline-registration-sim/ci-results/standard.json
python3 -B offline-registration-sim/run.py --history --report offline-registration-sim/ci-results/history.json
python3 -B offline-registration-sim/run.py --security-hint --report offline-registration-sim/ci-results/security-hint.json
python3 -B offline-registration-sim/run.py --fallback --report offline-registration-sim/ci-results/fallback.json
```

四个矩阵分别为 24 标准、18 既有历史、12 明确安全提示、32 全局候选继承场景；精确数量和 suite ID 由运行器校验。
新增两个协议矩阵统一使用合成 001/01，不按运营商决定行为。状态继承、去重和真实 AKA 前门禁另有实际 Rust 单元回归。
历史名称只是已有fixture标签，不得将某卡特例模型当作全局兜底正确性的证明。
增加验证应围绕请求继承、状态转换、授权/算法边界、次数/时间预算和错误分类。
预期拒绝是通过的负例，不是网络注册成功；模拟也不能代替无线/SIM/运营商订阅/真实安全通道。

前端依赖使用锁文件安装；隔离浏览器fixture必须拦截API并禁止连接真实设备。
真实页面调整需要验证原布局、选择保持、保存开关、blocked→ready切换及没有额外硬件请求，
不能用另一个临时页面代替用户界面验收。

## 6. 产物与发布

`VERSION`及三个依赖清单由`sync_release_version.py`同步。`scripts/pack-ota.sh`调用统一打包器，
程序、www、meta、同版本安装器/unit/设备资源和SHA清单来自同一commit。
普通push只生成artifact，`Publish Release`应skipped；正式发布需master上显式授权、新tag且完整门禁通过。
版本号不代表代码先后，必须核对commit、ELF、程序/前端和包SHA。

验证下载时：官方artifact digest → ZIP/包外SHA → 安全解包布局 → 完整文件SHA清单 → meta/ELF架构/commit。
校验来自可信发布方，不能把SHA当作数字签名；不执行未知下载内容以验证可信性。

合并docs-only收尾可以不重编译，但须证明构建输入与已验证代码提交相同，不伪装成新二进制。
旧Release/tag不能为清理分支被移动，旧制品不自动获得新安装器安全保证。

## 7. 设备验收

按[INSTALL](INSTALL.md)安排受控窗口，先看实时SIM/MM/注册/通话/管理路径，而不是重放旧脚本。
保留配置、运行库、归属账本和未知结果；备份与回滚范围明确。停止自己的服务不等于可以重启基带或清预算。

最小交付应区分：

1. 编译通过；
2. 自动化测试实际通过；
3. 产物摘要/版本核验；
4. 部署程序/HTTP/配置保持；
5. 当前SIM的初始注册；
6. 同会话自然续期；
7. 通话、短信、音频及故障注入。

上一步不能替代下一步。当前卡成功不能替代另一运营商；一次重新注册不能冒充自然续期。
本地历史凭据、完整日志、号码和数据库不得放入公开报告。

## 8. 文档规则

[docs/README](README.md)列出固定主题入口。新事实更新相应手册和简短HANDOFF；
历史验证只写[历史摘要](archive/README.md)，详细原始材料留Git历史及`.local`。
不继续新增日期命名的多份“当前交接”、重复Prompt模板或逐工具调用流水账。


## 无忧行托管：静态证据、凭据与实现边界

本页只描述用户提供的 APK 静态分析，不代表已经连接无忧行、注册成功或收发过业务。
目标是让 SimAdmin 接入托管收短信和接打电话；登录/号码绑定界面暂缓，由用户从自己的
root 手机取得现有凭据。计费独立于 IMS；接电话/收短信免费是用户给定需求，非实网验证。

### 1. 纠正旧勘察结论

此前“没有 SIP 字符串，所以是私有协议”结论撤销；`libmsc.so` 也不是已确认的通话媒体库。

- APK ZIP 内只有一个 `classes.dex`，但该文件内有多个合法 DEX：外层只有壳类，
  App 类在嵌套 DEX，Juphoon SDK 类在另一嵌套 DEX。只看外层漏掉真实调用链。
- `libidss-cn.so` 有保护/变换代码迹象，不能从名字认定为 IMS 信令栈。
- `libmsc.so` 导出 `MSPLogin` / `QISRSessionBegin` / `QTTSSessionBegin`：是 iFLYTEK 语音模块，
  其中 DTLS 字符串不构成无忧行媒体协议证据。
- `libmtc.so` 是 JusTalk/Lemon SDK，依赖 `libzmf.so` 及 Android 系统库；
  `Mtc_SgwDeliSipInvite`、`Mtc_SgwDeliSipSendMsg`、`receiveSipMESSAGE` 等正向证实 **SIP 网关支持**。
  这不等于普通 SIP 客户端可以直接连服务器；REGISTER、鉴权、SBC/realm 和传输仍未确认。
- HTTP 业务调用还出现 `requestWithEncrypt`，不能仅凭 JS 可读就认定控制面无需签名/解密。

### 2. App 登录与托管是否强绑定

**UI 可以分离，服务凭据不能省略。**

普通 App 路径 `JustalkManager.login()` 先检查 `SysApplication.isLogin()`，再读取号码身份、
`VoipStatus.sipPassword` 和 phone token。真正的 `JCClient.login(username, password)` 第二个参数
是 **getPhoneToken()**，不是解码后的 SIP 密码。

SDK 本身没有 App 登录页面依赖；`loginWithPassWord` 直接转发 SDK 登录参数。
另有 `loginUnLoginApp` 使用 `CallUserInfo.cstToken`，但已见调用用途包括免费/紧急呼叫；
不能把它解释为“未登录也能无条件托管本人 SIM”。

因此 SimAdmin 可以设计为“导入现有凭据 → SDK 会话 → 托管网关注册”，不实现登录 UI。
是否实际可用仍取决于服务端托管开通、凭据有效性、设备/会话绑定以及续期。

### 3. 手动取得的必要参数（只列名字，不提供用户或 App 内置值）

| 参数 | 静态来源 | 用途 |
|---|---|---|
| `sdk.account_identity` | `getAreaCodeMobileNum` | SDK 会话身份，保留原区号格式 |
| `sdk.phone_login_token` | SharedPreferences `wyx_phone_login_token` / `wyx_phone_login_token_key`；另一路 `CallUserInfo.cstToken` | SDK 登录凭据，不是 App 密码或 `SipPwd` |
| `sdk.application_key` | `JCClient.create` 参数 | 匹配 SDK 应用配置 |
| `sdk.server_address` | `config_key_server_address` | SDK 服务端配置，不要猜作 SIP SBC |
| `sdk.root_der_base64` / `sdk.tls_only` | `config_key_root_der` / `config_key_use_tls_only` | 匹配部署信任与 TLS 策略 |
| `sdk.device_id`（如已配置） | `config_key_device_id` | 保留原配置，设备绑定语义待验证 |
| `sdk.expires_at_unix_seconds`（若已知） | 后续确认的 token 到期信息 | 未知时留空，不虚构永久有效 |
| `hosting.sip_number` | `SipNumber` / `getAreaCodeMobileNum` | 被托管号码 |
| `hosting.sip_password` | `voip_status.sipPassword` 经 `decryptThreeDESECB` 后 | 网关 `SipPwd`，不能直接拿缓存密文替代 |
| `hosting.authname_prefix` / `hosting.channel_id` / `hosting.delete_int_code` | `AuthnamePrefix` / `ChannelId` / `DeleteIntCode` | 原部署网关参数，观察到均为字符串 |
| `hosting.enrollment_confirmed` | 自有账号已经开通托管的确认 | 导入参数不代替服务端开通 |

旧 `UserCallAssetsEntity` / `callAccountSipPassword` 是另一存储线索，格式需单独核实，
不能假定等价于新版 `voip_status`。已确认新版是 SharedPreferences 字符串中的 Base64 Java 序列化对象，
经 `ObjectInputStream.readObject` 取出；不在主机上反序列化任意 Java 对象。内部 `sipPassword` 仍需解码。
不把完整凭据贴进聊天或提交 Git；目前无需用户提取。

### 4. 托管调用链与实现进度

- SDK 登录：`JustalkManager → JCClientImpl → MtcEngine → Mtc_CliLogin`。
- 托管注册：`RegisterTimer → JCAccount.updateStatus → Mtc_SmgwUpdateStatus`；
  `STATUS_REGISTER=4`，`STATUS_UNREGISTER=3`。网关 JSON 是以上五个大小写准确的字段。
- 来电：`JCCall`；接收消息：同一 `JCClient` 上的 `JCMessageChannel`，
  App 接收回调进入 Realm/未读/通知。短信发送和服务端计费流程尚未确认。

当前模块 `connectivity::app_voice::providers::jegotrip` 已新增 **外部凭据和 SDK 适配契约**（原 d6381b3，本轮迁目录）：
配置输入脱敏、两阶段登录/托管状态、每线路/每代回调归属、到期/撤销使 readiness 失效。

`GET /api/non-3gpp/providers/app_voice/credential-contract` 只返回字段说明和实现边界，
不初始化 SDK、不读取账号、不发送网络请求；明确 `transport_implemented=false`。

**未实现** Linux SDK backend、凭据落库/API 导入、真实托管注册、收短信/接打电话、计费 UI。
模块不是已可用 provider，不自动注册，不改变下拉框可用状态。Google Voice/Telegram 槽位保留。
不能 dlopen APK 中 Android/AArch64 库冒充通用 Linux/AMD64 后端。

## 应用通话 provider 目录与下一阶段

`connectivity/app_voice/providers/` 下各 provider 独立目录：`jegotrip`、`google_voice`、`telegram`、`imessage`。
`registry.rs` 只共享能力/可用性接口，不共享登录、号码格式、信令、媒体或计费。IMS旧import保留re-export兼容。
后3项当前仅声明，iMessage只声明消息能力，不假扮运营商或语音服务。

无忧行 `controller.rs` 新增真实 SDK 适配调用序列（31d3739 已验证）：外部凭据导入不连网，start提交login；
SDK成功回调后才submit托管status=4，注销status=3后logout；请求/回调各有界超时，旧owner/错阶段忽略，
没有backend明确Unavailable。Build39626186439/Beta39626186611双架构成功，官方摘要测试ZIP各14原凭据+8controller+6registry测试逐名通过，363静态/mock成功。此处验证的是适配调度，不是SDK网络帧，API仍 `transport_implemented=false`。

独立Linux wire实现需继续分析用户APK的 `libmtc.so`：`Mtc_CliLogin`、`Mtc_SmgwUpdateStatus`、
`Mtc_SmgwSendMessage`、`Mtc_SgwDeliSipInvite`、`Mtc_CallAnswer`/`Mtc_CallTerm` 及其agent/序列化/TLS调用链。
DEX已证明JNI参数但不含登录/托管网络封包。必要时用户手动打开IDA；本轮未连接IDA。
准确导航符号/地址与JNI证据只留 `.local/jegotrip/next-native-analysis.md`，不硬编码到生产代码。

### 5. 下一阶段与验收条件

1. 继续确认 SDK 外线登录/网关帧、通知、注销、续期以及 Linux 兼容的实现路径。
2. 实现完整 backend 后才接入凭据持久化、线路配置和 provider 注册。
3. 对接来电、接收短信至现有 trunk/消息工作流；发送行为和资费独立，不继承 IMS 免费假设。
4. 用户有可用参数后，另行授权验证过期、掉线、并发设备、真实收短信及双向通话。

当前只有静态证据和契约测试；没有设备、服务端或资费验收。
Build 39600992021、Beta 39600991953 的官方摘要测试 ZIP 各 14 项新测试逐名通过，ARM64/AMD64 构建成功；
本机 361 项静态/mock 通过。不是真实 SDK 通信测试。详细方法引用与二进制指纹保存在
私有 `.local/jegotrip/offline-static-analysis.md` / `verified-dex-call-paths.json`，不提交 APK、DEX 或 SDK 库。
