# 开发、验证与交付

本页是代码工作流入口；架构见[ARCHITECTURE](ARCHITECTURE.md)，安装见[INSTALL](INSTALL.md)，
当前未完成事项只维护在[HANDOFF](HANDOFF.md)和[DEVELOPMENT_PLAN](DEVELOPMENT_PLAN.md)。

## 1. 分支与编译约束

- 无忧行模块：本机只解析 APK/嵌套 DEX/ELF、运行静态/mock、核验制品；Rust、前端及编译后的协议 peer 仅在两套 Actions 执行。APK、提取库、第三方报告、账号 token、网关密码、SDK app key/部署值只留私有目录，不提交。静态 credential-contract 不读账号或连服务；实际账号连接只由显式 `/start` 发起。详见[托管契约与操作](#无忧行托管静态证据凭据与实现边界)。

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

当前目标是**基础接电话、应答、挂机、双向音频与 Trunk，以及接短信、查询/删除、通知和原生回执**；外呼、发短信只保留接口。登录/号码绑定 UI 暂缓，由用户从自己的 root 手机取得现有参数。账号使用独立 `app_account` / `application_data` 线路，不借 SIM、五码、UICC、IMS/CS 或免费 WLAN 资费。

当前源码 `c6f0ba1` 已由两套 Actions、官方摘要产物和双架构核验；在基础入站整链、全生命周期来电收尾、可选断线清理和原 Trunk 排空之上，补齐DIRECT与batch长期接收的独立回收证明。原邀请→应答→Endpoint TCP-Mpath↔UDP RTP→挂机、认证 HTTP、Asterisk 侧 SIP 与通话期间 SMS 均有隔离执行证据，**不是已连接无忧行、设备部署或资费验收**。用户所述入站免费不替代实际账单证据。

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
| `sdk.selected_session_id`（原生账号运行必填） | `Mtc_CliApplySessId` / 原SDK会话配置 | login/swap方法第二串；不是phone token或返回ARC auth token，≤127字节；仅单独凭据结构兼容缺省，完整账号配置不允许缺失 |
| `sdk.client_config_decode_key` / `sdk.reference_decode_key`（可选） | 原Client属性PBE配置 / 原Common对象引用PBE上下文 | 仅ENC配置unwrap，二者不合并/不当pwd/token/SipPwd或auth证明，缺失保持缺失，不复制内置值 |
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

`GET /api/non-3gpp/providers/app_voice/credential-contract` 返回静态实现/参数说明，不初始化 SDK、不读账号或发请求；当前 `transport_implemented=true` **仅指显式入站实现**。`basic_hosting` 明列 profile、手动启动、plain-media opt-in、独立计费主体、实现的入站操作及 `default_sdk_client_implemented=false`；`automatic_cleanup_supported=true` 仅指下面的可选原运行清理，`automatic_reconnect_implemented=true` 仅指下文opt-in有界全新start（不是token续期，`renewal`仍为手工重新导入），外呼/发送均 false。它不是当前 readiness、服务可用性或收费授权。

Main 在创建原账号 manager 后显式安装 Voice/Messaging provider。Registry 的汇总 availability 不能互授能力，调用方分别检查 `incoming_call_ready` / `incoming_message_ready`；SMS-only 不授 Voice，反之亦然。只读描述不会加载配置或自动启动会话。Android/AArch64 库不经 `dlopen` 冒充 Linux/AMD64 后端。

### 当前基础托管配置与操作

#### 私有导入形状

创建请求为 `{"provider_id":"app_voice","config":完整配置}`。**上表的 `sdk.*`、`hosting.*` 实际位于 `config.credentials` 内**；不是请求顶层。`AccountConfig` 的类型定义在 `backend/src/connectivity/app_voice/providers/jegotrip/account_config.rs`，账号/profile/media 结构拒绝未知字段和不支持的组合；内嵌 trunk 沿用现有 `TrunkProfileConfig` 的兼容规则。以下是字段契约，不是可直接连接服务的示例参数。

| 配置部分 | 必须提供的形状与来源 |
| --- | --- |
| `config.credentials.sdk` / `.hosting` | 上表对应的原账号材料；`tls_only=true`、`enrollment_confirmed=true` 不代替真实服务授权；已知过期时间必须有效，未知保持 null |
| `profile.route`、`authority`、`enable_sni` | 仅 `direct_tls_carrier`；显式原 authority/SNI 与 `sdk.server_address`、原 DER 信任配对，不从 HTTP 域名猜 SDK 地址 |
| `profile.identity_type`、`domain_name`、`domain_id`、`application_id`、`device_info_cc` | 无 bootstrap 时 domain_name/domain_id/application_id 必填；CC 可选；来自原配置，不借 SIP 号码或 token 替代 |
| `profile.client_properties`、`include_device_id`、`login_routing` | 原 ClientId/Client 属性和路由；已导入 device_id 时必须保留，不能悄悄删去 |
| `profile.gateway_uri`、`gateway_metadata`、`poll_routing`、`logout_routing` | 原托管网关与路由属性，不借 SIM/P-CSCF 配置 |
| `profile.incoming`（接短信） | `{object_id, domain, receiver_kind}` 原绑定；可发布短信 readiness 的已实现 kind 是精确 `Sip.Request`，其他 kind 不猜成 SMS |
| `profile.call_incoming`、`profile.call_control`（接电话） | 原 `{object_id, domain}` CallClientEx receiver，以及含原 `id` / `domain` 的控制属性数组；与 `call_media` 一起提供 |
| 五个 `profile.*_timeout_ms` | `connect` 为 1..10000，`handshake` / `partial` / `io` 各 1..30000，`rpc` 为 1..60000 毫秒；都是显式有界预算，不推断 token 租期 |
| `config.bootstrap`（可选） | 独立 `account_authority` / `account_root_der_base64` / `account_enable_sni`、`routing` 与 1..30000 的 `handoff_timeout_ms`；不能同时手填 profile 的 domain/app/CC 字段 |
| `config.call_media`（接电话） | 独立 `authority` / `root_der_base64` / `enable_sni`、原 `routing` / `features` / `properties`，可选 `reference_decode_key`，显式 `audio_bind_ip`、`allow_plain_media=true` 和可选 `trunk` |
| `config.cleanup_on_disconnect`（可选） | JSON boolean，缺省 false；仅清理本 manager 已退休的原 run，本身不 start/renew 或清除未知前进程 owner |
| `config.reconnect`（可选） | `{max_attempts:1..16, initial_delay_ms:1000..600000, max_delay_ms:≤3600000}`；须 cleanup_on_disconnect=true，清理完成后同 revision 全新 start，仅断线/不可用/超时重试 |

属性数组的每项是 `{"key":[字节...],"value":[字节...]}`；receiver 的二进制字段同样用 JSON 字节数组，**不是字符串或 Base64**。`call_media.features` 每项是 `{"value":[字节...]}`。解码 key 只用于对应原 ENC 上下文；不合并 SDK、Client、reference 或 media 的信任/密钥。

`audio_bind_ip` 是 SimAdmin 实际可绑定且媒体对端可达的本地地址，不可为未指定/组播地址。它不是 native `Client.Host` / `Client.ArcPort`：后两者可能是逻辑 Router 地址/端口，不能当 OS RTP 目的地。支持的媒体为 raw `RseEx.SDP`、单音频 `RTP/AVP` / `sendrecv` / `rtcp-mux`，匹配 payload 的编码 RTP 原样转发（含已支持的 Opus passthrough，不是转码器）。压缩 SDP、SRTP、ICE/BUNDLE/P2P 或不支持协商明确拒绝；TLS 信令不因此降为明文。

#### 账号与业务 API

以下 `A` 表示 `/api/non-3gpp/accounts/{line_id}`，`C` 表示 `A/calls/{call_id}`。沿用 SimAdmin 现有认证；此处不提供凭据，也不执行真实服务请求。管理/通话响应设置 `Cache-Control: no-store`；`line_id` 和 32-hex `call_id` 是本地不透明标识，不能用远端号码/64-bit native ID 替换。

| 方法与路径 | 请求/返回语义 |
| --- | --- |
| `GET/POST /api/non-3gpp/accounts` | 列状态 / 单账号导入；创建不连接网络 |
| `GET/PUT A` | 脱敏元数据 / `{"expected_revision":N,"config":完整新配置}` 全量 CAS 替换；旧 IO/consumer 未释放不能替换 |
| `DELETE A?expected_revision=N` | CAS 删除；不能清除其他 manager 的原运行或强制接管 |
| `POST A/start`、`POST A/stop` | `{"expected_revision":N}`；start 的 202 只是提交，stop 按原来电媒体/Trunk、status3、SDK logout 与 owning Join 清理 |
| `GET A/inbox?limit=1..128` | 本地入站记录与回执层级，不导出 generic raw 或凭据 |
| `GET A/messages?limit=1..128` | 已严格分类的短信正文、sender、时间与来源；不是所有 generic 消息 |
| `GET A/notifications?limit=1..128` | 通知状态/次数/期限，不导出目的地、认证或 payload |
| `GET A/calls`、`GET C` | 当前 run 的通话状态、history_id、经约束的来电 peer 和本地媒体 SDP；无账号材料 |
| `POST C/answer` | `{"expected_revision":N,"sdp":"媒体对端的实际SDP应答"}`；≤16KiB，须与 offer 匹配；不是 WebRTC 浏览器音频接口 |
| `POST C/hangup`、`POST C/reject` | `{"expected_revision":N}`；只清理原通话，不借当前配置重绑旧结果 |
| `GET A/trunk` | 该账号原 Trunk 的只读状态 |
| `POST A/outbound/calls` | `{"expected_revision":N,"target":"目标"}`，有效请求仍返回 501 / `app_account_outbound_unsupported` |
| `POST A/outbound/messages` | 同上加 `"body":"正文"`；有效请求仍 501，不生成 outgoing history 或调用 SIM/IMS/CS |

`GET /api/non-3gpp/providers` 只列安装/实现元数据，整体 `unknown` 不等于某账号断线；实时接收能力看 `GET A` 的分别就绪字段。`hosted`、`incoming_call_ready`、单通话 `media_active` 与 `outbound_authorized` 含义不同；最后一个本阶段始终 false。来电的 native 接受回包本身也不授 audio-ready，必须等待原 Endpoint 和实际媒体 actor。排队 Connected 在消费时再次检查原权限、revision/有效期和物理媒体，取消不借新 owner 接管。

#### Asterisk/Trunk 与短信

- 使用 `config.call_media.trunk` 的现有 `TrunkProfileConfig`；无需虚构 modem 或将账号绑到原 WLAN/SIM。启用时配置真实 Asterisk host/port、本地 SIP port、用户名/认证、`incoming_mode` / `incoming_binding` 和 `codec_allow`。隔离回归覆盖 `static_peer` / `bound_pending`、双向 RTP、双方 BYE 和实际端口释放；不冒称已在真实 PBX 部署。
- `vowifi_only`、`allow_home_cellular_calls` 必须 false，`bound_immediate` 不支持；不能用 IMS 资费或提前应答策略给应用账号授权。Asterisk 发起的外呼被拒绝，接收就绪不改变此边界。
- SMS 进入既有 `sms_messages`，transport=`app_voice`、pdu=null，可用原 `/api/sms/list?channel_id={line_id}`、conversation/statistics 及 `/api/sms/message/{id}?channel_id={line_id}` 删除接口；`/api/sms/channels` 将接收 `incoming_ready` 与发送授权分开。interactive/automation/Trunk 三路发送都先拒绝应用账号 fallback。
- 原 receiver/owner/run 约束下，raw＋SMS＋事件＋通知计划同事务落库后才回复原 ticket。`batch_queued` 只是待传批次结果，`direct_submitted` 只是已提交，不伪称远端 ACK；新批次来电只对明确未入队的 `CallNotReady` 在固定预算内等待，不重发已提交 Inform。
- 通知沿用已有 SMS 规则/模板；最多 8 目标，原 rule/channel 指纹冻结目的地，4 并发、每 channel 单 claim、最多 5 次/24h。改同名目的地、删除、过期或未知发送结果不改投、不冒称成功；已发网络字节不能召回，不能保证远端 exactly-once。

#### 维护边界与证据

私有 `data.db` 仍是本机明文存储，不是加密保险箱；API 不导出账号配置。换参数先 stop 并确认 `io_cleanup_required=false`，再 CAS replace/start。取消 stop 后只能继续等待同一 owner 的剩余 Join；已完成任务不得再次 poll。正常退出后回收已投影私有 envelope 并保留共享 SMS；删除 SMS 清逻辑 raw/tombstone，但不保证 SQLite/WAL/备份物理擦除。

来电现在从 Checking/Ringing 起就保留唯一原 watcher，不等到媒体连接后才监听。原 ticket/account scope 断开、过期或媒体 scope 关闭时，经原 operation 锁结束历史与 Ended，不覆盖/丢弃 watcher、不重复 Release。即使 SDK 断线发生在振铃，也不会只丢端口却留下未结束通话。

设置 `cleanup_on_disconnect=true` 后，本 manager 的原 run 退休会触发最多20秒的自动清理（最多一次同 owner 清理重等），与人工 stop/关机在原 run 上串行；媒体、call任务、原 SIP driver、actor/controller 与 consumer 的实际 Join 后才提交 nonce fence/回收 envelope。它本身不发送新的登录或续期，不会按 PID/年龄清除未知前进程；只有另配 `reconnect` 时才在清理完成后由下节supervisor全新start。缺省 false 保持人工 stop 行为；配置仅存在数据库而没有本 manager 原运行时，不能自动清理或授 readiness。

原 Trunk 驱动通过私有 FIFO fence 确认先前 Ended 的本地处理/发送后再停止；普通事件观察者、替换/丢弃/落后 driver 不能假确认。失败显示 `app_account_call_trunk_delivery_unconfirmed`（HTTP503），不宣称远端已收到 CANCEL/BYE。SQL/历史结束失败保留原运行 lease，修复后只能重等该 owner，不重放已尝试的 Release。局部 socket 关闭不等于历史提交或原资源全释放。

`GET A` 在该 manager 持有 run 时可观察 `cleanup_on_disconnect`；自动清理后保留原 disconnected/expired 等原因和独立 `cleanup_error`。`io_cleanup_required` 表示本地原资源/fence 义务，`cleanup_error` 可能含局部已释放后的不确定远端/Trunk结果，两者不能互代。人工 stop 可重等/清除该原运行状态；最终租约释放后 `A/calls` 是运行期接口，不能再当作活跃旧call查询（`app_account_runtime_owner_unconfirmed`），已结束通话/短信历史仍在既有数据库/历史 API，下一次显式 start 才创建新 owner。

#### 退出进程运行回收（`1401be0f0add6f1bf89d17152a43c227b406f90f`（实现04c585d）已验证）

进程启动时（任何账号start与通知worker之前）对`data.db.app-voice-owner.lock`取非阻塞独占flock；之后注册的运行租约标记本进程实例。成功取得锁即证明此前打标记的进程都已退出（它们存活时都持有同一把锁），于是释放其遗留租约：未结束的应用来电写结束时间并发`call.ended`(recovered)事件，`sending`通知改为uncertain并清正文，按原规则回收已分类envelope，短信历史与未知raw保留，并发`app_voice.runtime_recovered`事件。不按PID/运行时长推断；锁被另一存活进程持有时不做任何回收；无标记的旧版本租约仍是`runtime_owner_unconfirmed`；恢复不启动任何会话。

Build40525320619/Beta40525320712、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各752 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，446static/mock，67warnings保留，Release skipped。首轮04c585d的Build有一项旧DIRECT测试在GC失败后人工stop收到原生如实的`app_account_disconnected`（同提交Beta及此前三轮均通过）；按既有"同owner再等一次"约定只修夹具（batch同类夹具同步），失败官方ZIP保留，生产代码未改。

#### 可选有界自动重连（`e5ef46dab3254016ab1ec2ba2aba316f4d407b1e`已验证）

可选`config.reconnect`（须同时`cleanup_on_disconnect=true`；`max_attempts`1..16、`initial_delay_ms`1000..600000、`max_delay_ms`≤3600000，指数退避）：只有本manager原run完成实际Join与fence释放后，才对同一已存revision做一次全新start（新nonce/owner/登录/托管），不是token续期或会话恢复。仅disconnected/backend_unavailable/operation_timeout可重试；鉴权/托管拒绝、过期、revision/owner/存储错误不重试。自身HostingAck后重置计数；人工stop/start、replace/delete、关机取消或取代待执行重启；未启用supervisor时报`unsupervised`。`GET A`在策略存在时给出`reconnect_attempt`/`reconnect_state`/`reconnect_error`。跨新run的重投与原生一样是至少一次，不承诺exactly-once。

Build40517095074/Beta40517094859、ARM64/AMD64成功，官方digest ZIP/CRC/逐名各749 Jegotrip＋7registry＋17HTTP＋46Trunk＋31通知＋4SIP＋7DNS/86，445static/mock，67warnings保留，Release skipped。

#### 长期收件与原回执回收

- DIRECT原去重是两个180秒接收窗口，严格超过窗口才单次轮转、重复magic刷新；RPC ID保持正序且不复用。pending原票据不按时间驱逐，128 pending、4096同时历史/burst及原128KiB身份字节预算仍有限。只有原actor实际响应write/flush后的私有frontier，在当前line/revision/run lease下同事务记录状态、推进不越过较低pending的水位并回收旧已分类副本。
- batch的`batch_queued`仍不证明已发送/ACK。必须取得原受认证swap回包，ACK精确`ack_next_batch-1-bit`所指、实际承载原result的epoch，才由同原consumer FIFO持久化稀疏已ACK身份。before-base前缀只在原2048-bit ring已不能重新投递、且不越过pending request或working/unACKed result时推进；未见乱序gap不被高序号误拒。
- 稀疏身份最多2048/account、8192全局，只有证明覆盖的前缀可以退休，不随ring移动任意清除；长期低pending可触发有界失败。DIRECT/batch分别保最近128项可回收短信envelope元数据与完整共享SMS；既有通知状态聚合/终态审计保留策略不变。unknown/pending/另一种协议或另一run不容量回收；已有raw/删除tombstone先于水位检查，已清退原票据不能复活短信。GC失败回滚并退休原scope，不重发已经提交的DIRECT ACK或已ACK batch result。原Join清理仅清本run的两类回收状态。
- 单账号配置/消息/任务仍有界；原32batch epoch/45秒未ACK、128 unknown/pending、无回收证明的512身份与全局16MiB envelope等安全预算保持。及时完成/ACK的已分类收件不再受终身128/512累计上限，**不是无限流量、无限磁盘、token续期或跨新run exactly-once**（重连见上节可选策略）。未知prior-process run仍为`app_account_runtime_owner_unconfirmed`，不按PID/年龄force-clear；默认全SDK/locator、remote principal/device租约和其他媒体仍单独据证补齐，不扩大成付费外发必做项。

最新源码 **`c6f0ba13bf2277910cc33834bcc1ea6fcd63f6b6` 已验证**：Build40499505183 / Beta40499505215、ARM64/AMD64成功，官方digest ZIP/CRC/逐名/source-run各 **744 Jegotrip＋7 registry＋17 HTTP＋46 Trunk＋31通知＋4 SIP＋7 DNS/86矩阵**，444 static/mock与10项SQL语法mock通过，67条compile warning保留，Release skipped。14新batch回归含同一个原manager/ARC/nonce600短信、2300ring滚动、partial ACK/低pending/乱序gap、删除/未知与GC rollback、129条后ACK/GC失败不重放；DIRECT600/原音频Trunk等回归保持。Build artifact12733490387 SHA`18d49e76cd6a6a6798041529e28992c63c6b437d65e2cdbd2892073cc35bfa7b`，Beta12732930975 SHA`de9e9b66e51b5c532b270a4851ac7ae6b009087a87b93daddfd7bb04484665c0`；见`verified-native-c6f0ba1.json`。DIRECT前源码cb33653两Actions各730项独立通过；首9d0原TLS夹具只有inbox fence缺runtime lease，生产frontier拒绝正确，仅夹具按真实runtime注册并增强SQL/原ACK断言，加缺lease负例，失败官方ZIP保留，不弱化归属。公共docs-only收尾构建输入同c6f0ba1，不假称另一个已验二进制。

前源码 **`b13223ab8f8515a9e27b84a64c12f0a68809e90a` 已验证**：Build40458126579 / Beta40458126681、ARM64/AMD64成功，官方digest ZIP/CRC/逐名/source-run各 **711 Jegotrip＋7 registry＋17 HTTP＋46 Trunk＋31通知＋4 SIP＋7 DNS/86矩阵**，438 static/mock通过，65条compile warning保留；Release skipped。新增12 Jegotrip、4原Trunk FIFO和1认证HTTP实际执行，原双向音频/接短信/通知/归属回归保持。Build artifact12713424118摘要`d2bcd976c01d43d6a1a8bcdef33332cdf163926074c5a18788765b7b56e3eabc`，Beta12713408030摘要`07212f43c97a1ed7e9e5598d202b0a8e4acf72e7239b18fe788cf0e6555611a1`；清单见私有`verified-native-b13223a.json`。首e78a7b1两Actions编译成功但一新fixture在最终lease释放后错误unwrap当前call query，已仅修fixture为原对象/SQL历史与旧query应拒绝的断言，失败ZIP/全文保留，不弱化生产owner/TLS/Join/FIFO。

前源码 **`d0a2b675329489c72542bb3b3d382e02b6eda15a` 已验证**：Build40445960947 / Beta40445960949、ARM64/AMD64成功；官方 digest ZIP 各逐名匹配 **699 Jegotrip＋7 registry＋16 HTTP＋42 Trunk＋31通知＋4 SIP＋7 DNS/86矩阵**，435 本机 static/mock 通过。六项新 admission/batch 与强化的双 Join 取消回归均实际执行；首轮通过，保留65条编译warning，不声称零warning。官方测试 artifact：Build12708308718 / Beta12708966079；详细官方摘要、CRC、源码/run/矩阵指纹见私有 `verified-native-d0a2b67.json`。Publish Release skipped；这不是程序包部署或服务验收。文档收尾须保持与该已验源码相同的构建输入，不假称新二进制。

用户提供的第三方 DOCX 只作线索：其 APK SHA/大小与本地样本不同。表格/工具说明已只读核对，当前原 `libmtc.so` 在 IDA 的 metadata 与本地/实际 APK 库字节相同；raw SDP 分支存在不代表远端实际选择。HTTP 加密/签名或 Android 信任策略不替代 native wire/授权证明。不照搬报告密钥，不运行其中服务示例；若需要 JADX/其他新工具，先说明版本和用途，由用户手装。

## 应用通话 provider 目录与下一阶段

`connectivity/app_voice/providers/` 下各 provider 独立目录：`jegotrip`、`google_voice`、`telegram`、`imessage`。
`registry.rs` 只共享能力/可用性接口，不共享登录、号码格式、信令、媒体或计费。IMS旧import保留re-export兼容。
后3项当前仅声明，iMessage只声明消息能力，不假扮运营商或语音服务。

## 历史协议组件与验证

以下记录是**各指定提交当时**的实现与证据。旧 `transport_implemented=false`、未安装 provider、来电/媒体未接等状态不覆盖上方当前契约；中间失败、修正与原生格式细节保留供维护复核，不能直接重放旧命令或扩大本次授权。

无忧行 `controller.rs` 新增真实 SDK 适配调用序列（31d3739 已验证）：外部凭据导入不连网，start提交login；
SDK成功回调后才submit托管status=4，注销status=3后logout；请求/回调各有界超时，旧owner/错阶段忽略，
没有backend明确Unavailable。Build39626186439/Beta39626186611双架构成功，官方摘要测试ZIP各14原凭据+8controller+6registry测试逐名通过，363静态/mock成功。此处验证的是适配调度，不是SDK网络帧，API仍 `transport_implemented=false`。

独立Linux wire实现需继续分析用户APK的 `libmtc.so`：`Mtc_CliLogin`、`Mtc_SmgwUpdateStatus`、
`Mtc_SmgwSendMessage`、`Mtc_SgwDeliSipInvite`、`Mtc_CallAnswer`/`Mtc_CallTerm` 及其agent/序列化/TLS调用链。
DEX已证明JNI参数但不含登录/托管网络封包。用户已手动打开并确认目标 `libmtc.so`，本轮通过MCP仅只读分析，未修改IDB或执行库。
准确导航符号/地址与JNI证据只留 `.local/jegotrip/next-native-analysis.md`，不硬编码到生产代码。

### 原生参数编码与通知适配（1d8d042已验证，type0前缀后续修正）

- `rpc_arguments.rs` 实现 `updateStatus.SipGatewayPub.SipGateway` 的参数：type0流标记、BE16版本表计数1/唯一版本0（不是major/minor）、真实SDK UE URI的BE32字节长度、BE32 map计数及键/值字节串；map按键字节长度再按字节排序，不能用Rust默认词典序代替。原SDK五字段及string `SipFlag=4/3`保留；128KiB/NUL/空凭据检查是本地安全边界，不冒认服务器协议限制，Debug脱敏。
- `native_events.rs` 接受可信backend已解码的**本地SDK通知**，不接受API或未认证网络帧。native `updateStatus`先验证RPC结果为0和返回boolean为true，才发Ok通知；通知本身没有另一个boolean字段。注册/注销共享通知名，必须用提交时cookie捕获的owner和本地operation分开，不能收到通知才重绑当前owner。
- controller在通知到达时检查凭据到期和monotonic deadline，过期优先、重复tick不改变原因；同名线路删除/重建有独立incarnation。先拒绝旧/未ready来源，再有界复制原始body/from/time字节，不lossy转UTF-8、不猜3GPP TPDU。发送成功、status2成功或ReconnectOk都不能确认本次托管或授予通道ready。
- 两套Actions显式选中17凭据/会话、19controller、9codec、10native事件共55项回归由Build39747922739/Beta39747922813双架构通过，官方SHA测试ZIP逐名及四矩阵86场景核验；本机365静态/mock通过，报告`.local/jegotrip/verified-native-1d8d042.json`。不实现ARC/TLS完整封包、token续期、真实收短信或通话；provider仍不可用。静态证据只留`.local/jegotrip/ida-mcp-first-pass.md`，不提交APK/SDK库或运行偏移。

### RPC封装、批次与结果（5d4bbb0已验证）

后续IDA构造函数/流长度/输入工厂/汇编交叉确认：type0字节必须在版本号前，不能只写BE16版本；1d8d042的55项CI通过不能证明原错误前缀等价于SDK，已独立留证并修正测试。请求参数前缀为版本表计数1/唯一版本0（原major/minor名称已纠正，字节不变），result stream不重复表；terminal错误字符串必读，非空错误即使boolean=true也拒绝，后继可选property map保持脱敏。

- `rpc_stream.rs`：type0/BE16/BE32、有界byte string/metadata map、长度优先键排序、重复/截断/trailing拒绝；binary `__magic`不套UTF-8/NUL政策。128KiB流/64KiB字段/128entry为本地安全政策，不是假服务器限制。
- `rpc_envelope.rs`：request为type0+positive call id+metadata+method+raw args；reply为type0+原call id+result code+raw result。均无socket/握手或readiness副作用。
- `rpc_batch.rs`：`swapCalls.AccountEntry.Account`入参使用loginSession返回的user id与**原选SDK session ID**、六个原顺序opaque词、signed int32索引request/result maps；返回ARC auth token属于metadata `token`，不是第二方法串。5d4bbb0此串旧名称/fixture误导已在本轮纠正，其CI只验证旧形状不证明选值正确；外层bool/code不能代替每个托管结果，也不自动重试。`rpc_login.rs`检验非空返回id/token，不猜expiry或刷新。
- `rpc_pending.rs`：prepare记录原owner/注册或注销operation、单连接递增id与独立receive scope；reply仅完成原pending一次，旧连接/retired/duplicate/注销回包不确认注册，非零/坏result撤ready。controller回归串联实际encoder→envelope→tracker→ack→独立messages channel→原byte消息，仍为mock不是公网互通。
- 95模块回归由Build39767535250/Beta39767535255两套Actions/双架构及官方SHA测试ZIP逐名核验（17session/20controller/12arguments/10native/8stream/6envelope/9batch/6login/7pending），四矩阵86场景通过，366静态/mock通过。报告`.local/jegotrip/verified-native-5d4bbb0.json`，只读核验器`verify_native_ci.py`不运行本机Rust。新`.local/jegotrip/ida-mcp-rpc-transport.md`记录原函数/构造/排序/结果/下层TLS取证，不提交库或偏移。

已定位外层FF-FF/NetStream及`DataKeyTls=6`包装；TLS对象被嵌入自有帧，不能把TcpStream+rustls直接宣称兼容。下层socket/路由/信任/握手/provisioning与真实业务仍未接通。

### ARC会话与Common分片（684ef73已验证offline子阶段）

- 四份材料严格分层：导入phone token是login Common metadata `pwd`；原选session ID是login/swap方法第二串；login返回ARC token是后续metadata `token`；托管另用SipPwd。`SdkCredentials`可选字段缺失保持缺失，ARC层必须显式提供，≤127字节且不接受控制字符；不借device ID/密码或内置默认值替代。
- `rpc_bootstrap.rs`只编解码getAppConfigs：true/zero code、opaque properties、AccountEntry/AccountAlive/UserEntry及footer；仅AccountEntry必须非空，不猜端点、TLS信任或到期。`rpc_login.rs`按native colon-first身份归一化且要求显式domain，不虚构账号种类。
- `arc_session.rs`拥有原owner/连接scope、递增不复用call id、login/auth/close状态。原配置覆盖`__appKey`/`__domain`/`__app`/`DeviceInfo.CC`，phone/auth metadata不能由caller覆盖，未解ENC材料拒绝，ForceFlag保持0，不做设备强制接管。只匹配成功login才发SdkLogin，bootstrap不授auth，失败batch不交付nested结果。
- 只读审查发现独立scope交接可误用另一ARC的同序号结果，已补私有构造`OwnedBatchReply`捕获原owner/scope；每ARC一次领取`HostingRpcTracker`并在提交前绑定，失败/退休/关闭/drop共享terminal scope使pending和留存旧reply失效。nested托管必须在swapCalls内；direct-envelope helper仅offline/mock，不能当native送socket路径。
- `transport_packets.rs`纯codec需要一个**完整下层packet**：Common request0/reply1/ping2/release4/pong14；TLS模式分片阈值16382，首片BE32写原payload长度，分割length+4且末两片平衡。FF FF 06仅包装opaque TLS bytes；压缩Bzip2、legacy XOR/Kx/降级、非法fragment/oversize拒绝，组装错误永久关闭。不是任意TCP read或TLS read的分界器。
- `arc_protocol.rs`只offline组合RPC与plaintext packet；先scope核验再parse，借用reply同步交付，controls不授登录/hosting，release/未实现server request关闭，login退休/失败清理partial buffer。需真正受信TLS引擎提供已确认的plaintext packet边界，不把carrier或控制码当成功。
- 代码`684ef73cdfdde2bed1c262a95bd6e871c14a6bd3`已由Build39812859460/Beta39812859585实际各139模块回归（18凭据/21controller/12arguments/10native/8stream/6envelope/10batch/7login/13pending/6bootstrap/11arc session/10packets/7protocol）、ARM64/AMD64构建验证，Release skipped；两套官方SHA绑定测试ZIP逐名精确139及四矩阵86场景通过，本机368静态/mock通过。新四module filter实际执行，报告`.local/jegotrip/verified-native-684ef73.json`；`verify_native_ci.py`只下载/核验，不运行本机Rust。保留compile dead-code/unused warnings如实，未接真实backend不消除unsupported声明。来源与私有review/evidence留`.local/jegotrip/`。
- 下一项真实backend仍包括Router/NetStream/NetSsl socket与完整boundary、部署参数解密/可信端点、batch epoch/ACK/dedup调度、server-object入站、持久化/provider/runtime/API/trunk/SMS/calls/media/计费/续期/并发。`transport_implemented=false`/provider unsupported/外发授权false不变，本轮无设备或服务端访问。

### 离线批次调度与入站消息（8efcd49已验证，非真实backend）

用户当前没有可提供的实网配置，要求继续离线/理论完善。该授权不包括部署、访问手机/服务端、拨号或短信发送；缺少配置不是停止静态还原的理由，但也不是用默认值虚构可用性。

- `batch_scheduler.rs`六词已由native发送/解析/初始化/回包交叉还原为本地incarnation、exclusive next batch、保留批次bitmap、peer incarnation/next/bitmap；ACK bit对应`next-1-bit`。32批/45s保留且重发原嵌套序号/bytes，ACK只删transport history，原hosting结果/通道ready另验。不会在本机启动loop或连接。
- 2048序号/64词ring去重按32stride滑动；本地安全子集忽略before-base旧序号、拒绝非正/wrap、peer incarnation变化失败关闭，不复制native unsigned-negative跳转reset导致旧业务重放。native retention丢旧history，本实现显式失败关闭而非默默丢单或声称完成；本地entry/128KiB保留总量与snapshot/serial/seq耗尽均有界。
- 原ARC只领取一个scheduler；scheduled snapshot携原owner/scope/identity、active+encoded状态，未编码不commit、已取消旧snapshot不能提交或编码、编码后取消关闭scope。单次prepare期间不变更working maps；queued但未submitted请求不得被无关poll结果确认。scheduled回包须通过ACK/epoch校验与原submitted sequence过滤后才交tracker，不可由raw外层bool绕过。
- `rpc_incoming.rs`确认`recvOnlineMessage.MessageReceiver.Client`为版本stream＋receiver-kind byte string＋property map＋raw stream；返回type0/bool/terminal footer。仅安装原显式object/domain/receiver-kind且不接受wildcard；未知method/缺receiver不猜响应，unsupported关闭。sender/time原byte保留，`Sip.MessageGw.Oid`不自动改全局gateway，不能凭此认定所有消息均SMS。
- `arc_protocol.rs`可处理明确配置的direct server request；需要同scope已auth、精确id/domain、binary `__magic`，正call id不复用。bounded magic缓存不evict/replay，压力关闭；原私有ticket由consumer确认后才编结果。native双180s magic窗口/所有wildcard/别名/并发尚不声明等价。TCP read/TLS read仍不是已确定packet边界。
- `offline_backend.rs`只组合已有ARC/packet→scheduler→原hosting callback/入站ticket，**不实现HostingBackend或启用provider**。direct/batch message票据都绑定原scope，私有payload/ticket配对、借用completion重试、消费一次；收到消息不授channel、不写DB，不在消费成功前ACK。新回归覆盖login/pwd/token、nested ACK与message、invalid ACK不得交hosting、dedup、wrongscope/无配置/未知method、退役ticket。
- `8efcd49c7befe3e6395f7b97834bcdb57e3f393f`已由Build39834770425/Beta39834770520实际各172 Jegotrip模块回归、ARM64/AMD64验证，Release skipped；官方SHA绑定测试ZIP逐名172及四矩阵86场景核验（新增17scheduler/7incoming/8offline＋1reply encoder），370本机静态/mock通过，新3suite过滤器实际执行。报告`.local/jegotrip/verified-native-8efcd49.json`，保持原warnings证据、不冒称完整backend。审查6超时只返回ticket-by-value回压丢失的部分发现，已借用修复并Actions回归；不能算完整审查或互通证据。私有`.local/jegotrip/batch-inbound-offline-evidence.md`保存原函数/汇编，未执行目标库。
- 补充语义复核：type0后BE16 `1,0`由native版本表解析器明确为计数1/唯一方法版本0；不是major/minor pair。旧shape/139通过证据保留，本轮字节不改，只纠正注释与手册；unsupported版本/多版本表仍failclosed，不把形状正确当所有版本互通。
- 仍需下层Router/NetStream/NetSsl socket及完整boundary、受信TLS/ENC provisioning/端点、业务kind/SMS可靠分类/入站落库/线路API/provider/trunk/call-media/费用/续期/并发；未把这些未实现项降为“只剩实机”。`transport_implemented=false`、provider unsupported、outbound false保持，只有明确支持子集通过Actions后才能报offline完成。

### 下层PacketSender与Router安全子集（902f4f5已验证，非真实backend）

- `packet_stream.rs`实现native PacketSender route的BE32原packet byte length（不是长度+4），接收缓存跨任意split/coalesced input并按精确长度循环拆包。零/超限长度、超过128帧/单input上限、EOF partial显式失败；每次调用的后段错误不交付该次前段输出。128KiB加固定routing overhead为本地政策，不冒称native64MiB或服务器协商。
- `router_packets.rs`独立8-byte头的三BE16词/byte6/kind-part位、opaque tag/低2flags/path words/两address；地址按mask固定长度，无解析公网端点/猜反向source-destination。正常part1、过MTU严格比较后的floor/ceil两半part2/3独立相邻序号；本地只支持一个有序pair，未知控制/扩展/TLV/legacy decoration/out-of-order/wrap拒绝，非全部native可靠调度。
- `packet_transport.rs`由原ARC一次领取，显式选择direct FF FF 06或Router path，TX/RX绑定分别捕获并精确匹配。输入先owner/scope核验再parse，完成Router重组/route后只输出opaque TLS bytes；不调用on_plaintext_packet/RPC/controller/DB，frame/carrier不授auth/channel。错误/EOF/drop关闭原共享scope，不重绑新同名session。
- `frames_for`/`bytes_for_tls`是checked snapshot，不是可撤销IO lease：close只阻止新access，旧borrowed/copied bytes仍可读。只读review指出旧说明过强，已补明确回归；真实writer/TLS actor必须串行final-consume与close/取消排空，不跨await拿旧snapshot。本模块没有该live队列或socket能力。
- SHA一致本地ELF/FDE/ARM64证明原sender/receiver/BE32排序，不执行库；IDA MCP当前WinError10061，不自行启动/加载或改IDB。TLS辅助任务超时仅留反汇编，main独立核验；完成的source review无本机构建、broker一个坏event如实保留；主代理随后补发协调消息因同时带recipient/requestId被工具拒绝，未送达，不算额外确认。mode3直接SSL_write Common packet且SSL_read可pipeline记录，**无内层BE32**，不把任意rustls reader当packet。
- 新10stream/11router/14owned-transport＋1offline组合回归由`902f4f54042e76e4ed330efedaa23f892106895e`两Actions实际执行：Build39912321845/Beta39912321855/ARM64+AMD64成功，Release skipped；官方SHA ZIP精确各208模块回归及四矩阵86场景核验，373 Python静态/mock通过，三新suite非零过滤器实际执行。报告`.local/jegotrip/verified-native-902f4f5.json`；Build12483402678 SHA256`164a41f41893f87bdf89f28506ae68fb9eaae8dde34cdca8b1fc7c3aeae064ff`、Beta12483058183 SHA256`5cecd81e3155a5ead18686d5e60c01a511885f9b96498016688500ef820c0fa6`。保留55条compile warnings（14 Jegotrip关联节选）不掩盖offline接口尚未live接线。证据`.local/jegotrip/resume-network-framing-evidence.md`；无本机编译，本步ELF资料足够，后续只有确需IDA才告知用户具体文件。
- 剩余真实socket/NetStream/runtime、受信TLS/ENC provisioning/端点、record-aware明文边界、SMS可靠分类及落库/provider/API/trunk/call-media/费用/续期/并发；provider unsupported、transport_implemented=false/outbound=false保持，无设备/服务/部署。

### 可信TLS与Common明文流（784cd3c已验证，非完整backend）

- `tls_engine.rs`只操作内存rustls ClientConnection，不开socket/查DNS/接服务。显式imported base64 DER root（≤16384bytes）、原authority及SNI策略，stock chain/time/signature/EKU/name验证后额外exact DNS/IP SAN；无系统/public roots fallback、mTLS/weakTLS、earlydata或session resumption。握手只提供wire输出，TLS verified不发SDK/hosting/channel callback，验证前encrypt_packets拒绝。
- `tls_plaintext.rs`按原type0 request/reply字段计数/长度自定界；native mode3固定16382、N+4末两块平衡chunk plan，独立fixture覆盖record/任意read split/coalescing、binary metadata、非法counts/trailing等。无猜内层BE32，未知compression/另threshold/kind失败关闭；既有CallAssembler和完整envelope重复key/id/bounds仍执行。
- 标准cipher record缓存跨carrier，先有界type/version/BE16record length，再交真实rustls验证；inner plaintext再次独立分界。writer一完整native packet≤16383一次write，rustls max_fragment_size包含5-byte header已按源码校正；handshake output也由bounded Write控制上限，不以rustls application bufferlimit当全部内存上限。EOF/close_notify/坏cipher/非法明文关闭原scope，无恢复明文。
- 原ARC一次TLS或raw PacketTransport slot；私有VerifiedPlaintextPackets才能交OfflineBackend新接线，错scope/旧owner在parse前拒绝。不夸大snapshot可撤销性，live writer/TLS actor final-consume/close/取消排空仍需实现；固定handshake(≤60s)与整体partial(≤30s)deadline，流量不重开计时，回退monotonic钟关闭，非SDK token寿命/续期推断。
- shared `core/tls_identity.rs`仅提升可见性，算法与旧SIP helper完全一致；旧sip_tls_identity wrapper保留两原测试名/路径，额外shared两测试加入Actions。新10plaintext/19TLS已实际执行，共237 Jegotrip；`784cd3c861be50b04bd80a7d8091de19d3d7a12e`由Build39937771208/Beta39937771426/双架构成功，官方SHA精确237＋2sharedSAN/2原SIP及86矩阵核验，376Python/static/mock通过，Release skipped。报告`.local/jegotrip/verified-native-784cd3c.json`；Build12495390365 SHA256`5b4efa48b61531db90c96872b47944714a21bebc159f095f6fcacd5bfd6ccf2b`、Beta12494633662 SHA256`d9e878a3a4d43609f060522a52a78a916de97bde65100cc7671ce488dd7fbfec`。57compile warnings/16Jegotrip节选留证，不运行本机Rust/openssl peer。Actions测试进程生成临时证书并做真实内存TLS握手，不提交私钥/root材料。
- 两辅助设计/source review480s超时无结果不算验证，main独立核对原ELF与rustlsAPI；首次本机静态wholeprovider HTTPS禁令命中非法authority fixture，改中性scheme保留负例；fixture原ARC过早drop/非Clone concat/吞reader errors已主代理修正，原失败保留`.local/jegotrip/static-tls-first.log`。证据`.local/jegotrip/tls-offline-evidence.md`，本步不需要用户IDA。
- 仍无live socket/NetStream/全Router控制/可靠调度，ENC provisioning/端点导入/credential落库/API/provider/trunk/SMS分类/通话媒体/费用续期并发，transport_implemented=false/providerunsupported/outboundfalse保持；本阶段不能冒称全部无忧行已复刻。

### 原会话IO actor与关闭门禁（cd0684e已验证，非完整backend）

- `transport_actor.rs`仅start caller-provided `AsyncRead+AsyncWrite+Unpin+Send`，不开TCP/不DNS/dial，不是installed HostingBackend。单actor拥有原IO+TLS+OfflineBackend，32command/128event及128KiB参数/有界wiregroup；原metadata/route/owner不在收包时重绑，TLSverified/sentreceipt不是登录/hosting/channel确认。
- RpcConnectionScope保留ArcAtomicBool身份，加sharedMutex<()>+Notify。close在gate内终止；write/flush每次NONBLOCKINGpoll同gate核验，Pending释放再重新检查，无await/阻塞poll/reentrantclose可放锁内。毒化终止、Notify enable-before-check防wake丢失；不宣称已kernel接受字节可召回，snapshot仍不可撤销，只新的actorpoll被阻断。
- sendpartial按原cipher帧offset续写，不重新encrypt/分配businessid；commandoneshotcancel/eventconsumerdrop/close/IOTLSRPC deadline选择terminal、IO对象drop/原ARC清理/queuecompletion丢弃。RuntimeCloseReceipt io_released只有原JoinHandle真正结束，取消closeawait不先takehandle，后继可再等待原cleanup；不把atomicfalse当硬件或远端注销完成。
- receive实际TLS验证→Common→原batch/hostingcallback或消息，pendingdeadline按原ARC是否pending退休，不靠ACK/control。originalconsumer完成private ticket，borrowedAPI+内部capturedcopy不会队列满丢票，pendingconsume仍一次；hosting元数据与outerrouting分别显式传入，不默认空object/domain或guessedkind。deadline(≤30sIO/60sRPC)包含TLSpartial，timerMissedTickSkip不重开计时。
- 新14actor tests用duplex+内存rustls核查真实login/原callback/hostingbatch+directbinarymessageconsumerreply、gate与close换poll/offset/poison/wake/cancel/DropGuard/IOrelease/queuepressure等。`cd0684e9a0e2c54203082f5050c1aefd328ea549`由Build39958776382/Beta39958776314/ARM64+AMD64成功，官方SHA ZIP精确各251模块回归＋4shared/retainedSIP及86矩阵核验，378Python/staticmock通过，Release skipped。报告`.local/jegotrip/verified-native-cd0684e.json`；Build12503962642 SHA256`b2c81e95d910f4b9c49203b4d1cf14852f3924e60a8699972512583bcde429f8` / Beta12503744444 SHA256`4e6bc26934f8b462d141ae3a68ee6a1e6313665c6bc1e0e8bc169a7ffeaaf3fe`，保留59compilewarnings/18Jegotrip节选；无本机Rust/target/openssl执行。
- review辅助300s超时返回partial两项test编译缺口（共享fixture可见性、future借用temporaryarray），main均修复并独立补真实metadata/deadline/cancel/ticket语义；不是完整review。一次WSLBashE_UNEXPECTED未执行即失败，后继同cwd复试正常，无目标/设备操作；原证据`.local/jegotrip/actor-offline-evidence.md`。本步无需IDA。
- 仍缺fullRouter/NetStream可靠控制/outoforder/MTU与ENC provisioning端点、provider/persistence/API/trunk/SMS可靠classification/callmedia/费用续期并发；transport_implemented=false/providerunsupported/outboundfalse不变。callerIOactor测试不能升级为真实Linux公网互通或完整托管可用。

### 部署ENC与原会话bootstrap引用（`ed6fb33`已验证）

- `legacy_config.rs`据native Pbe salt8||DESCBC+PKCS5/no-newlineBase64，MD5(password||salt)再999digest轮，first8oddDESkey/last8IV。仅配置legacyunwrap，非TLS/SIP密码算法/授权；两个contextkey显式输入，无builtin/password替代。Malformed/oversize/notmultiple8/重复嵌套/wrongpadding失败，不复制native弱key -2未检查unsafe分支或静默cipherfallback。CBC无MAC，wrongkey可以偶然通过pad，不能凭decode成功信任引用/账号。
- RustCrypto des0.8.1只该codec使用，包官方checksum`ffdd80ce8ce993de27e9f063a444a4d53ce8e8db4c1f00cc03af5ad5a9867a1e`经archive与registry比对；existingcipher0.4.4启zeroize/锁已有zeroize1.9直接使用。手工lock由metadata核验，真正--locked编译仅Actions。不增加OpenSSL生产链接或弱TLScipher，密钥/digest/raw/解码buffer、ownedreference/bootstrap/plan/Secret/auth与pendingselector均drop-timezeroize。
- `object_reference.rs`recoverfacet slash identity、@adapter$locator或:semicolon endpoint/explicit -h/-p。Groups []/{} /<>、boundeddepth，grouped$只在外层split/两field校验，未知native groupedlocator runtime不宣称等价；quote/backslash/hiddencontrol/proxy/range/hb/z/substitution默认不猜。地址支持原DNS/IP字面值，parse_scheme保留但仅rawtcp兼容当前innerTLS PacketSender；tls/udp/http/tlsp/svarc未降级。未安装root/替authority/dial。
- `OwnedBootstrapReply`只能matched getAppConfigs originalpendingid/owner/scope+zero/true/footer成功构造；裸AppConfigs codec不能伪造cap。properties/AccountEntry/Alive/User/footer私有copy/Debugredacted/Dropwipe，material_for oldscope拒绝；snapshot不被scopeclose物理撤销。OfflineBackend转交Bootstrap配置event；actor新增explicit bootstrap命令在TLSverified后实际提交，receipt非login ACK，未知字段不授channel。
- `deployment.rs`先原scope再decodeAccountEntry(Common key context)，parse候选；Client属性按需另key decode，missing留空；原Alive/User引用独立非deliveryready。候选是数据，后续真实connector还需原importedendpoint/authority/root/provision binding，不从plaintext成功替换当前IO。两optional Secret字段兼容旧imports，≤4096bytes/不empty/control、不注册可用provider。
- 新9PBE/12reference/8deployment＋1actorbootstrap=281回归＋4shared/retainedSIP由Build39983918744/Beta39983918718/ARM64+AMD64成功，官方SHA ZIP逐名及四矩阵86场景核验。381Pythonstaticmock通过；独立OpenSSL DES-CBC oracle/公开NIST块/固定MD5KDF/确定interior-padding与pad0/pad9/内存真实TLS bootstrap→privateconfig→explicitlogin全部仅Actions运行，manual lock已--locked验证，无本机Rust/target/设备服务。本步ELF/FDE足够，无需用户IDA；Release skip、57compilewarnings/17Jegotrip excerpts保留，报告`.local/jegotrip/verified-native-ed6fb33.json`，Build artifact12515211829 SHA`c93507de3d304f3373cb280dad5392169630d17e953adee39b248b80a9a9a6db`，Beta12514343458 SHA`41ece8747963e9e75b60bcb88350e67db6ed1d101b744a867c0446ba1af45c4c`。
- Read-only review辅助partial后provider ratelimit，3findings已main修（group$拆成未闭合fields、retainedownedcopieswipe、padding负例不足），不计fullreview/compilepass。InitialstaticFDE absent脚本失败改boundeddecode，批处理timeout只留partialfile非验证；证据`.local/jegotrip/deployment-offline-evidence.md`。fullRouterNetStream/provision/rsaAuth/endpointtrustconnector/providerpersistAPI/SMSclass/trunk/callmedia/费用续期并发仍未完成，unsupported/outboundfalse。

### 完整 backend 的下一实施关口（持续）

按[开发计划](DEVELOPMENT_PLAN.md#p0无忧行-backend-的实施顺序与原生证据关口)继续完整Router/NetStream和default locator/原可信映射。`aa87c18`已验证本地per-account runtime排他和CallEx数据、原matched bootstrap到单独TLS account scope/IO handoff保留；独立账号、Smgw文本/持久通知和原run回收保留；这些不是默认native或call/media/trunk可用。global provider/Channels/paidoutbound仍false，不冒充UICC/PDU/免费VoWiFi。本轮原IDA同SHA/base0再次只读核对，58C98C/58F6F8/580BA4复核原字段/独立pwd语义；不自行加载或改IDB，当前无需重开。

### 原运行排他与CallEx数据（`aa87c18`已验证）

- `account_runs`新增持久per-account runtime exclusion，line/revision/run nonce与原fence同IMMEDIATE事务admit；`AccountManager`与Store CAS保护原lease，跨manager/SQLite不重复start/替换/删除或无owner stop。Reopen显示cleanup-required/owner-unconfirmed，legacy live不以PID/age自动清退。原actor/controller与consumer都Join的私有proof才可按原nonce/revision CAS，并与GC同事务release；失败/stale proof不清新owner，原bootstrap两句柄/通知/历史fence保留。
- 本地per-account排他不识别别名/重复导入的remote principal，不承诺device全局并发/服务端独占或crash-owner恢复。未知持有者保持blocked，尚无force-clear/猜TTL/renewal。
- 只读IDA `484788`确认为CallClientEx dispatcher：479DE8 onInvited为原BE64 id/peer string/map，479C2C/47963C为Released/Accepted BE64/map；477074 accept为版本表[0]/BE64/map，4757E4为zero/bool/map/Common footer。E70B6C逐字节证实64bit高位，不从旧EH合并的E70C88 decompile猜入口。新`rpc_calls`保留opaque数据、拒绝未知method/version/重复/截断/trailing及Debug redaction，native id不截为ARC/direct/batch i32。
- 这是data codec：没有call receiver/agent安装、original-call capability、shared pending/result归属、answer/release runtime、自动ACK、SDP/crypto/endpoint推断或媒体/Trunk就绪。5FA47C有输出变换分支，5FCA0C的CONNECTING和60851C hooks不是完整media证明。仍须原call owner/反取消/资源joins、独立action/cost/principal/device/renewal与媒体支持，不能用bool/codec或本地lease代替。
- Build40207983533/Beta40207983422/ARM64 AMD64 success、Release skipped；官方SHA ZIP各精确**544 Jegotrip＋31service/模板＋13真实HTTP/auth＋4SIP＋7DNS/原86矩阵**，415static/mock、58warnings/17节选，首轮两Actions通过。新增6SQL/3原manager+actor/1HTTP和9CallEx/1BE64；旧external-revision fixture加Store拒绝后显式SQL注入，原断言不削。首90s本机static外层timeout原日志保留，240s budget重跑完整PASS，无本机Rust/peers。
- Build artifact12604921666 SHA`9da01b2a5a2c7bd79af8146d4bce7701725cb681713224546c2d0475ea54b65e`；Beta12604792393 SHA`e096bd713cdc2898c70c635545ad7db2cdceabdad5cc1a83b5805ba914e6418f`。证据`verified-native-aa87c18.json`/`runtime-exclusion-call-evidence.md`。未设备/服务/部署/NV/IDB写，UI/unknown资料不动，globalprovider/Channels/paidoutboundfalse。

### 原bootstrap到单独账号TLS handoff（`7cc8b8f`已验证）

- `runtime_provision.rs`只能从原owner/source/call-id成功`OwnedBootstrapReply`构造bounded plan：DomainName原map，DomainId/AppId/可选CC按显式Client key，AccountEntry按Common key；缺CC清空。原destination physical scope和imported AccountTrust身份固定，source关闭后copied data不是新source IO lease，也不能授权另一root/scope。完整reference仅接受已支持direct TCP，authority/root/SNI独立导入，locator/alternate transport不当默认端点。
- `runtime_backend.rs`预先捕获两个ARC/TLS scope，原source连接只做getAppConfigs；保存plan后等待原source owning actor真实Join，再connect/attach新的account scope，不修改活动IO。总handoff deadline涵盖全流程，原TLS clock不重开；原pwd/appKey/selected session/DeviceId/ClientId与supplied routing保持，bootstrap不发SDK/hosting/Channels callback。取消/失败保留两owning handles，logout原cleanup barrier完成后才released。
- 私有`AccountConfig.bootstrap`要求单独trust/routing/预算，manual domain/app/CC不能与bootstrap混用；旧version1 JSON遗漏bootstrap仍explicit直连，manual缺required字段失败。私有存储/CAS与认证/no-store HTTP保持，Derived字段不覆写原配置，不自动start，不导出root/密码。data.db仍本机明文，不声称加密或SQLite/WAL物理擦除。
- Build40202373634/Beta40202373625/ARM64 AMD64 success，Release skipped；官方SHA ZIP各精确**525 Jegotrip＋31通知service/模板＋12真实HTTP/auth＋4原/sharedSIP＋7DNS/原86矩阵**，410本机static/mock，58compile warnings/17Jegotrip节选保留，首轮通过。新增10provision/11真实双TLS与AccountManager/5config＋2HTTP；不同root/SNI/scope、未真实login不callback、source Join取消重等/总deadline、错scope/未知reply及旧JSON兼容实际执行。
- Build artifact12602133480 SHA`22fe5810c469061073d9b0002443f0d89c2e9ef027978c38002e3b56fedbec1d`；Beta12602128285 SHA`bdcd088e6108bf276e253bfa599dbaf9ea3c0bd804ec646c5df141afe0b9770f`。证据`verified-native-7cc8b8f.json`、`bootstrap-handoff-implementation-evidence.md`。初WSL GitHubErrno113用opt-in Windows系统curl的有界HTTPS证据读取恢复，不改digest/commit/清单规则；warning初extract未strip ANSI的零计数已按官方test-compile.log纠正，不冒称无warning。辅助review API400无findings，research partial不当signoff；无本机Rust/设备服务/部署/NV/target/IDB写、UI和未知数据未动。
- 仍需完整default locator/NetStream/Router/Channel生命周期、call ingress/accept/terminate/media/Trunk、独立action/cost/principal并发与renewal、有证明的跨进程恢复及共享历史政策；不把上述支持子集降为仅剩实机。

### Router/Channel 原生边界（`65fad23`已验证）

`native_router.rs`支持kind2/3数据controls、2048-bit seq统计（不drop duplicate DATA）、u16双向邻片wrap与局部缓存；`native_channel.rs`支持显式8/10header、LSB ACK/mask、preceding-data gap31、Skip16bit runs及BE16/FE-FF records。没有完整liveNetStream/congestion/FEC/选路/可信connector；旧orderedcodec/TLS/actor保持，不迁入nativeXOR/Kx/resume/requeue取消bytes，不授provider/ready。

Build40017848477/Beta40017848512/ARM64 AMD64、官方SHA测试ZIP各315＋4共享/原SIP/86矩阵通过，383static/mock，Release skipped，59warnings/19Jegotrip excerpts保留。源码inspection发现半片expiry与owned/growingbuffer未wipe，main已allinputexpire、TransportBytes/PartialMessage Dropzeroize、bounded几何extend_wiping/预reserve/精确join；不读freedmemory或撤销callercopy。源审查不是编译/实网signoff。证据`.local/jegotrip/native-router-channel-evidence.md`。

### 原会话SDK注销/命令归属（`60c6c64`已验证）

正常logoutSession使用returned userid＋原selected session ID两个方法字符串和metadata token；external0/boolean/mandatory footer私有reply只有原owner/scope/callID才能完成。Actor收到true后结束原IO，owning RuntimeHandle再Join才io_released；commands-only clone不能换scope/复制任务或报告release。取消立即旧pollgateclose，invalidowner/authrouting在guard前拒绝。Controller callback退休仍保留原backend_owner供注销/retire/失败cleanup重等，不伪造未start unregister，不重启/重导入/重放未知注销。Property/Login/Binding/Gateway/bootstrap key清零，不撤回caller/kernel已接受字节。

F556A0/F537FC已证实DATA/Start payload与nestedSkip descriptor；Smgw510E3C/510464登记`Sip.Request`，JCNotify/MessageChannel/Realm为text/可选millis，本地UUID不是remote ID，不猜PDU或复制日期fallback。Build40026210652/Beta40026210599/双架构、官方SHA测试ZIP各341＋4共享/原SIP/86矩阵通过，384static/mock，Release skipped，60warnings/20Jegotrip excerpts保留。原literal guard失败与辅助Azure400无结果保留，不冒充review；证据`.local/jegotrip/native-teardown-evidence.md`。

### 原生托管注销结果（`8e9a570`已验证）

STATUS_UNREGISTER不再丢弃内层结果或以physical Swap/外层batch ACK冒充成功。私有OwnedUnhostingReply只来自原owner/connection/inner sequence的0/true/footer；false/malformed不发HostingAck。独立内层deadline在外层RPC退休后继续覆盖所有IO和晚到结果；完整aggregate/queue admission先于cancel guard，未提交的invalid/backpressure不关闭原IO。成功保持原IO供SDK logout，失败/取消关闭旧gate，只有owning Join才证明release。

同响应true结果先暂存，original unhosting_pending/deadline保持到automatic outgoing及visible event publication全部成功，再复核expiry/scope发oneshot；队列满/IO失败不先报告成功。request/owner/all退休及closed-scope drain清除未交付completion，队列有界、overflow关闭旧scope；不撤销已借出copy。

Build40045409934/Beta40045409943/ARM64 AMD64 success、Release skipped。官方SHA测试ZIP逐名各355 Jegotrip＋4共享/原SIP与原86矩阵通过；Build artifact12541590493 SHA`0d595ad1f0e56885723c839fb0b5d383d236e7181fe8b292bebb1d2234eb3873`，Beta12541302207 SHA`7d271ec06948e4634ab7e15f9d15968ce8828820e865d4b028fadafe6f4337c6`。387本机static/mock，60warnings/20Jegotrip excerpts保留，无本机Rust/openssl peer/target/设备服务。两readonly source reviews完成，main修三具体缺口；后审查无新具体问题而非执行/全backend signoff。8项新增回归包括aggregate/queue、所有IO deadline/late ACK、同响应event backpressure/verified TLS writer失败、completion退休/overflow；原拒绝测试在explicit close前断言自动retire。两工作流现有native_router/native_channel/rpc_logout及actor/tracker filters实际执行。证据`.local/jegotrip/verified-native-8e9a570.json`、`unhosting-ack-evidence.md`、`unhosting-resume-review.md`；transport_implemented=false/provider unsupported/outboundfalse保持。

### 5. 下一阶段与验收条件

### 显式原会话连接与HostingBackend（`d593551`已验证）

`runtime_connector.rs`只接显式direct tcp reference＋原authority/root/SNI；adapter/locator和其它scheme不当tcp默认值。ConnectedIo原owner/physical scope/request字段先捕获、consume精确匹配，Root/cert仍由原TLS引擎验证；PreparedRuntime在connect前拥有原ARC/TLS/scope，attach不mint scope或重开TLS期限，取消prepared立即retire。shared Hickory的hosts/配置准备在gate外，实际网络future每poll同旧scopegate，4秒DNS/10秒connect组及地址数量有界；没有系统hostname fallback或active IO替换。此轮不执行真实连接，只有Actions memory/loopback peers。

`runtime_backend.rs`实现真正HostingBackend原connector/actor调用、bounded eventpump与poll、SDK成功原callback后scheduler配置、register submission/原status3 inner semantic/SDKlogout/owningJoin分层，closed ARC negative理由保留而不开放positive ACK/messages/channels。原owning handle在attach/spawn前拿slot，失败/取消cleanup保留原Join；poll admission只与logout起始串行，不能阻塞status3所需后续poll。记忆原result/已确认release不重放unregister/logout，不允许旧owner新start，borrowedconsumer票据无提前ACK。Profile值是显式原配置数据，不默认ClientId/DeviceId/selected-session；configured DeviceId不能丢弃，Common reference ENC只给显式key解码。此版本尚不自动bootstrap/locator/connection handoff、不装provider/Channels true/外发授权。

Build40057801164/Beta40057801104/ARM64 AMD64成功、Release skipped，官方SHA测试ZIP各逐名394 Jegotrip＋4共享/原SIP、7既有DNS、86矩阵通过；Build artifact12546610143 SHA`9ee166d0c7e016cd9590b0285c524ab2295c60a72982ee97a0c4186a7f3501bb`，Beta12546077296 SHA`4cf5a2c2005e1348c30b497769ebc2b9991d3caa9033af40f06194409dd19f6d`。391static/mock，57warnings/17Jegotrip节选保留。首lookup_host static失败未豁免guard而改shared prepared DNS；首9809ae7 Beta E0618 callback shadow以fixture-onlyd593551修复，failedZIP/annotations保留。代码helper只写两文件+16source tests，main补2并修terminal/ENC/device/Join slot/poll死锁与logout race；不当review/实网signoff，readonly辅助timeout无findings。两新module filters已实际执行，证据`.local/jegotrip/verified-native-d593551.json`、`runtime-backend-implementation-evidence.md`。仍完整NetStream/路由/正常provision原endpoint绑定、accountline私有存储API、durableSMS/callmedia/trunk/独立费用续期并发缺口，transport_implemented=false/providerunsupported/outboundfalse。

### 独立应用账号与通用消息耐久入站（`d666f10`已验证）

- `account_config.rs`保存原SDK/网关两层凭据和显式`direct_tls_carrier` profile；identity/DeviceId/ClientId/selector/authority/root/引用与binary属性分开，严格验证后才落库或start，无默认值。私有JSON version1、256KiB预分配zeroizing buffer，Secret/config无公开Serialize；SQLite页缓存/解析器临时内存不保证擦除。**data.db为机器本地明文私有存储，不是加密保险箱**；备份/目录权限须保护，API无凭据导出。
- `account_store.rs`独立`app_voice_accounts`，随机本地`app-account-`＋32hex ID不冒充远程ID或ICCID；cap64、IMMEDIATE事务、严格revision CAS、失败/溢出不更新。原provider id始终`app_voice`；早期新schema误写`app-voice`已原子迁移，保留配置/版本/时间及失败回滚，不更改既有产品id或未知对象。
- `account_runtime.rs`由AppState拥有，startup不连接/恢复cached ready。显式start构造真正NativeHostingBackend，await/连接前保留原run/controller/Join；取消同步retire旧scope，stop按原STATUS3→SDKlogout→Join，实际释放后才能replace/delete/restart。配置revision变化终止旧scope而不重绑定，原negative reason保留；global provider/Channels/外发仍false。
- `account_inbox.rs`保存generic原body/sender/time及原receiver object/domain/kind；line/revision/run nonce与actual direct call-id或batch peer-incarnation/sequence定义本地dedup，NULL不等于empty，同key不同bytes/binding拒绝。旧schema新增binding列保持NULL unknown，不从当前profile填补历史。raw＋`app_voice.inbox_stored`脱敏event同事务commit，之后才提交原借用consumer ticket；这是持久接收generic bytes，不是SMS类别、内容编码或媒体证明。
- 64KiB/字段、128行/账号、2048全局、16MiB原消息与receiver累计配额；不驱逐已持久化未知消息腾位置。native queue压力保留16项/1MiB原消息票据，重试不重复event；direct只记`direct_submitted`，batch只记`batch_queued`，不是peer ACK。回执标记SQL失败保留原data但不重发已消费ticket；close竞争下已接收观察可能历史提交，不授新scope，已发字节不宣称可召回。显式删除账号同CAS事务只清其私有inbox，脱敏历史event保留。

| 管理接口（沿用既有认证，响应`Cache-Control: no-store`） | 行为 |
| --- | --- |
| `GET/POST /api/non-3gpp/accounts` | 列脱敏状态；单账号创建要求`provider_id: app_voice`与完整`config`，不start |
| `GET/PUT /api/non-3gpp/accounts/{line_id}` | 只读metadata；全量替换要求`expected_revision`，活跃IO先停止 |
| `DELETE /api/non-3gpp/accounts/{line_id}?expected_revision=N` | CAS删除；不借新配置清理旧owner |
| `POST .../{line_id}/start`、`POST .../{line_id}/stop` | JSON `expected_revision`；202只代表原RPC提交，stop需原IO释放 |
| `GET .../{line_id}/inbox?limit=1..128` | 仅本地row id、unclassified、回执层级和时间，无raw/body/sender/凭据导出 |

Build40114399262/Beta40114399090/ARM64 AMD64成功、Release skipped；官方SHA测试ZIP逐名各**445 Jegotrip＋6真实production-router/auth HTTP＋4共享/原SIP＋7DNS、原86矩阵**核验。Build12568140922 SHA`b1f1aed6cf7ee83072a809334677c54b25bba74401c1976c506d4c775332a142`，Beta12567913303 SHA`3b907263daef37e544346c447c52be92067dd914b9606774130d24b78ac3f39e`；401本机static/mock，56warnings/16Jegotrip节选保留。REST quota403后从官方公开run页面取同artifact-id/digest、job页面核对双架构succeeded，结合先前已保存实际REST success；仍逐名/矩阵/源码fingerprint严格审计，非第三方自报hash或仅绿灯。证据`verified-native-d666f10.json`含每官方网页URL/文件SHA，`app-inbox-implementation-evidence.md`说明边界。

前账号阶段3b7356d为423＋5HTTP/core76493e3为442＋6HTTP，各有独立两Actions/SHA证据。首d5e26a2 E0753/E0624仅修测试include注释/私有scope观察；第二b309c0f core421通过但3HTTP因错误provider id返回501，已纠正真实schema/fixture并加迁移回归，四份失败ZIP/annotations全部保留，未削断言。源码贡献辅助partial限流、只读review Azure资源400没有结论；main独立复核/Actions不冒充完整review或实网。无本机Rust/前端/peer/注册模拟，无设备/真实服务/部署/NV/IDA写；未改UI，仍需SMS可靠分类/业务promotion、默认native路径、媒体/Trunk、独立费用续期并发。

### 已证实的 Smgw 文本接收与共享SMS历史（`0390692`已验证）

- 原SHA/base0只读IDA确认`510E3C`登记精确`Sip.Request`，`524104/524110→523EC0`将原body、`Sip.Message.From/Time`装入type5 JSON字符串；新追踪`585608/250BF4/61DFF4/24F738`是保留字节/JSON转义，不是PDU/base64解码。原DEX `JCNotify→JCMessageChannel→Realm`将text/userId/sentTime放入短信history；本地randomUUID不是remote id，`Sip.MessageGw.Oid`的native全局副作用不复制到Linux跨账号路由。
- `smgw_text.rs`只识别**原捕获receiver精确Sip.Request**的严格UTF8、明确非空有效sender与可选十进制Unix毫秒。保留Unicode/换行及看似hex/base64/JSON的原文字，不二次解码；其他receiver、无sender、非法UTF8/控制/时间保留unclassified。时间限1970..9999普通RFC3339，实际缺失时用本次原receive timestamp排序并明确`timestamp_source=received`、`sent_at_unix_millis=null`，不伪造send time或复制Android历史日期改now的行为。
- `account_sms.rs`再次核对原raw id/line/revision/receiver/body/sender/time及当前账号版本，只在原raw事务内写共享`sms_messages`与`app_voice_sms_projection`、脱敏`sms.received`；transport=`app_voice`、独立app-account line、pdu=NULL/local sms_id。raw＋generic event＋SMS＋SMS event全部commit后原consumer回复；任一SQL/event失败整体回滚，不把raw-only算SMS成功。相同原投递不重复history/event。
- `GET /api/non-3gpp/accounts/{line_id}/messages?limit=1..128`经过认证/no-store，只回已分类文本与sender/时间源；generic raw与凭据不导出。既有SMS列表/conversation/statistics按本app line工作；SMS通道kind=app_account、availablefalse、不编造ICCID/物理槽位身份。UI未改，interactive/automation/trunk SMS发送都在IMS/CS lookup前拒绝`app_account_outbound_unsupported`，不继承VoWiFi免费策略。
- 原SMS删除/清空的SQL trigger同时清除对应private raw的逻辑body/from/time列，并保留原scope digest/receiver tombstone；原票据相同retry不复活用户删除的内容，异内容仍冲突，其他line/transport不受影响。删除账号清private envelope而保留共享SMS history，用户可再通过原SMS API删除。**这不是SQLite空闲页/WAL/备份的物理擦除保证**。
- 本阶段仅history/持久事件接线，**通知外发worker尚未接入**；provider/Channels/paidoutbound仍false。128inbox/账号、2048总量与16MiB政策仍含已分类/删除tombstone，历史压缩/清理仍需完成；无已证实远程delivery id，不能承诺跨新run/重连全局exactly-once。默认native路径、真实calls/media/trunk、独立费用/续期/设备并发继续open。

Build40121560400/Beta40121560403/ARM64 AMD64 success、Release skipped，官方SHA测试ZIP各精确**471 Jegotrip＋8真实HTTP/auth＋4共享/原SIP＋7DNS/原86矩阵**核验，403本机static/mock；56warnings/16节选保留。Build12571244585 SHA`e1e05f44630b3d9ad89d4cbe8e65a7d3da287453b18a5dfcd19543980d9c0064`，Beta12571179295 SHA`148a0c7935d930ec83c03ac385df8493511c1c98cf413cc8397f795e0a5b61b9`。新增8parser/14SQL/4原native peer＋2真实HTTP，覆盖分类拒绝、同事务失败无ACK、sender/received时间、跨账号/删除tombstone、原共享SMS/API与三路外发拒绝；本阶段两Actions首次通过，不把前445成绩当新471。证据`verified-native-0390692.json`、`smgw-text-classification-evidence.md`、`smgw-text-implementation-evidence.md`。无本机Rust/前端/peer/注册模拟，无实际设备/无忧行服务/部署/NV/目标执行/IDB写，未知文件不清理。

### 原run回收、持久通知计划及关机（`c55f773`已验证）

- `account_runs.rs`在IO前登记原line/revision/run nonce，raw事务/回执标记都要求原run仍live。scope.close、phase、PID/年龄、direct_submitted/batch_queued都不是释放证明。原controller/actor真实释放后，释放controller锁，再取消并await原consumer Join；句柄留在原run的async slot。私有`RunQuiescence`才允许一个IMMEDIATE事务写released fence并回收该run的已投影envelope；失败不设finalized，replace/delete/restart继续阻断，取消重等不丢Join。
- SMS和时间来源不随私有envelope删除；detached投影随后删SMS时清除，已删tombstone在原run结束后回收。未知raw及对应fence、无结束证明的legacy/orphan保留，不造进程退出证明。128unknown/pending、512所有identity/账号、2048全局/16MiB有界；原direct pending+seen128及batch规则不改。解决正常结束run后的终身堆积，不是单连接无限消息、自动重连或崩溃恢复。共享SMS用户历史政策另行处理。
- `account_notifications.rs`与service adapter在**SQL外**捕获配置，纯snapshot按现有SMS规则/模板/quiet policy生成已渲染目标；queue计划或skip决定与raw/SMS/event同事务commit后原ACK。无目的地明确skipped，旧历史legacy不追溯转发，原投递重复不重规划。app-account上下文不查modem/own number；label为应用消息而非CS。模板膨胀分配前有界检查，原替换顺序不变。
- 每SMS最多8目标/256KiB计划、单body128KiB；完整rule/channel配置canonical SHA256冻结目的地/认证/策略。序列化临时有界zeroizing、不输出配置。dispatch只取一次当前配置并核对指纹；同channel-id改目的地/停用/修改规则取消旧job，不把旧内容转投新对象。
- 私有queue限2048条/16MiB、4任务工作池与每channel单claim，generation CAS约束完成/重试/取消，最多5次尝试/24h到期；rate/quiet延迟不消耗发送尝试。周期扫描不依赖broadcast/已裁剪event。pending可恢复处理，未知旧sending不擅自接管，uncertain/expired不当成功；有限重试可能重复，**不承诺远端exactly-once或跨设备全局并发**，原legacy通知worker不冒称已重写。
- 删除SMS/账号取消并清queued payload，旧finalizer不复活；已admitted发送不可召回，SQLite/WAL/备份无物理擦除保证。`GET /api/non-3gpp/accounts/{line_id}/notifications?limit=1..128`沿既有认证/no-store，只回本地job/SMS id、状态/尝试/期限，不回正文或目的地指纹。成功日志仅原line/rule/channel id与通用label，不写sender/正文/远端响应/凭据。
- Main保留通知worker和账号shutdown任务句柄；shutdown拒绝新start、取消pending start、等待原actor/consumer及持久fence；超时明确unconfirmed，不伪造release。通知只由已提交job和用户配置触发，不自动开始SDK会话。UI/globalprovider/Channels/paid SDK outbound不启用。

前ecebec1回收子阶段由Build40128580374/Beta40128580295/双架构/官方SHA核验484＋8HTTP；最新c55f773由Build40135247656/Beta40135247579/ARM64 AMD64 success、Release skipped，官方SHA测试ZIP各**499 Jegotrip＋31 service（17原模板/规则＋14应用planner）＋10真实HTTP/auth＋4SIP＋7DNS/86**核验；406static/mock，58warnings/19节选保留。Build12575949542 SHA`d0988aaf163e3580c03297a0af22e3170147ead1701f0a5f5fd0807221eabca7`，Beta12576518726 SHA`10f748d2e262d6e9414d463fdadb3b57faa4fe89c0036e3bf09191f44f053f60`。首7bc018b main两处Duration缺导入令测试/双架构E0433，c55仅补全std::time路径，原失败两ZIP/annotations保留、断言不减。

只读helper建议和2个service文件源码贡献都不是执行signoff；main独立审阅/接线、补边界测试与Actions。原IDA新增证实DomainId/AppId的Client解码、原DomainName，以及`accept.CallServerEx.CallEx`原64bit id/属性与版本重试；Locator string注册的3不当wire版本。`native-provision-call-frontier.md`记录后续，仍缺完整默认native/媒体/费用、跨进程owner恢复和共享SMS历史政策，不是只剩实机。无本机Rust/设备服务/部署/NV/target执行/IDB写、未知数据未清理。

1. 继续正常SDK provisioning/locator、完整NetStream/Router控制及可信原连接映射。
2. 补有依据的orphan进程恢复、共享SMS历史政策和必要其他编码，不提前授globalprovider/Channels。
3. 对接真实call/media/trunk；发送/资费、续期/设备并发独立，不继承IMS免费假设。
4. 用户有可用参数后，另行授权验证过期、掉线、并发设备、真实收短信及双向通话。

当前有静态证据、契约及offline codec/owned协议测试，但没有设备、服务端或资费验收。
Build 39600992021、Beta 39600991953 的官方摘要测试 ZIP 各 14 项新测试逐名通过，ARM64/AMD64 构建成功；
本机 361 项静态/mock 通过。不是真实 SDK 通信测试。详细方法引用与二进制指纹保存在
私有 `.local/jegotrip/offline-static-analysis.md` / `verified-dex-call-paths.json`，不提交 APK、DEX 或 SDK 库。
