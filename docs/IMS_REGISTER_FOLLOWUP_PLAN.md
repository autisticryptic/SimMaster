# IMS REGISTER 后续开发与验收清单

> 建立日期：2026-08-28<br>
> 项目：SimAdmin<br>
> 分支：`feat/ims-register-423-negotiation`
> 基线提交：`0444d58 feat(ims): complete register status-code fallback and terminal abandonment`

## 1. 文档用途

本文档用于接续此前的 IMS REGISTER、运营商配置投影和续期稳定性改造。

维护规则：

- `[ ]` 表示尚未完成或尚未验证。
- `[x]` 只有在代码完成且对应验收通过后才能勾选。
- 每次完成一项时，在该项下补充日期、提交号（未提交时明确写“未提交”）、测试命令和实机证据。
- “已写入工作树”不等于“已完成”：没有完成审阅和回归的代码仍保留在待验收区。
- 不要改动或清理当前未跟踪的抓包、发行包和分析目录，除非另有明确任务。

## 2. 问题背景与当前结论

现场现象是两个 IMS profile 都能收到 REGISTER `200 OK`，但其中一个 profile 的被叫会直接进入语音信箱。日志还显示周期性 refresh 失败并触发接入重建。

已定位的直接软件问题：REGISTER、NOTIFY、MESSAGE、MWI 等共享同一条 SIP 信令通道；旧实现会把 refresh 等待期间收到的任意 SIP 帧都当成 REGISTER 响应解析。收到请求帧时产生 `sip_status_line_invalid`，导致已经可用的 IMS registration 被错误拆除。

当前修复方向：

1. 用 Call-ID、CSeq 数字和 `REGISTER` 方法识别当前 REGISTER 客户端事务。
2. 非本事务帧暂存并交还会话循环，不让 REGISTER 事务误消费。
3. refresh 失败时先尝试兼容性降级，不立即重建整个接入腿。
4. 运营商数据库中的显式 `omit` 必须压过代码默认值。
5. PANI/CNI 必须最终来自真实接入上下文，而不是静态 RAT 或全零 cell-id。

## 3. 已写入当前工作树的功能

以下内容已经写入源码；代码层和自动化回归已按本轮记录更新，真实硬件/运营商网络验收仍保留在待办区。

- [x] REGISTER 事务键包含 Call-ID、CSeq 数字和 REGISTER 方法。
  - 位置：`backend/src/connectivity/core/register.rs`
- [x] REGISTER 等待循环跳过 provisional response，并有数量上限。
- [x] 非当前 REGISTER 事务帧进入 requeue，而不是当作状态行解析。
- [x] VoLTE、VoWiFi TCP、VoWiFi UDP channel 支持 fresh read 与 requeue。
  - 位置：
    - `backend/src/connectivity/core/access.rs`
    - `backend/src/connectivity/modems/ims/volte/channel.rs`
    - `backend/src/connectivity/modems/ims/vowifi/channel.rs`
- [x] VoLTE refresh 增加有限候选阶梯：当前形态、去掉接入网络信息、再去掉 Route。
  - 位置：`backend/src/connectivity/modems/ims/volte/live.rs`
- [x] VoLTE PANI 发送由 profile 三态策略控制，不再由 MMTEL 特征隐式强制开启。
- [x] VoLTE CNI 增加 profile 开关和报文生成路径。
  - 位置：`backend/src/connectivity/modems/ims/volte/sip.rs`
- [x] carrier catalog 支持布尔字段使用字符串 `omit` 映射为显式 false。
- [x] `security_agreement: "omit"` 映射为 `sec_agree_mode = "disabled"`。
  - 位置：`backend/src/connectivity/modems/ims/vowifi/carrier_catalog_v7.rs`
- [x] MMTEL Contact feature tag 的通用补全框架已存在，不需要另起一套实现。

已知已经通过的定向测试：

- [x] `register::tests`：23/23（2026-08-29 统一回归）。
- [x] `volte::sip::tests`：36/36（2026-08-29 统一回归）。
- [x] `volte::live::tests`：53/53（2026-08-29 统一回归）。
- [x] `carrier_catalog::v7::tests`：12/12（2026-08-29 统一回归）。

补充：2026-08-29 已完成统一回归；完整测试和仍未完成的真实环境验收见第 4 节及第 14.7/14.8 节。

## 4. 当前改动稳定化：代码审阅与回归

### 4.1 完整代码审阅

- [x] REGISTER transaction filtering 的超时语义已统一到单次 deadline；无关帧不会重新延长完整等待窗口。
  - 已确认 provisional response、无关帧分别计数，并拒绝缺失 Call-ID/CSeq 的错误匹配。
  - 完成日期：2026-08-29
  - 自动测试：`cargo test --bin simadmin register::tests -- --test-threads=1`（23 passed; 0 failed）。
- [x] requeue 的所有权和顺序已审阅。
  - FIFO 顺序、TCP/UDP/protected UDP channel 的 `into_parts` 保留，以及 channel 转换后的帧回交均有覆盖。
  - 完成日期：2026-08-29
  - 自动测试：`volte::channel::tests`（4 passed; 0 failed）、`vowifi::channel::tests`（6 passed; 0 failed; 1 ignored）。
- [x] requeue 队列具备帧数和总字节数上限，避免跨事务无限累积。
  - 当前 channel requeue 上限为 64 帧 / 4 MiB；超限按实现策略丢弃并记录诊断。
  - 完成日期：2026-08-29
- [x] refresh CSeq、Call-ID、认证 nonce 和 Security-Verify 生命周期已审阅。
  - 423、401/407、421/494 及兼容性候选切换后，事务键和安全状态不会跨候选污染。
  - 完成日期：2026-08-29
  - 自动测试：`volte::sip::tests`（36 passed; 0 failed）、`volte::live::tests`（53 passed; 0 failed）。
- [x] teardown/unregister 路径已复用事务过滤，避免注销事务误消费其他 SIP 帧。
  - 完成日期：2026-08-29
- [ ] VoLTE 与 VoWiFi 对相同 profile 字段的解释完全统一。
  - 仍需第 7 节的 schema、`omit` 保留和最终 SIP 报文端到端工作；当前只完成了相关代码路径的局部修复。

### 4.2 编译、格式与回归

- [ ] 全局运行 `cargo fmt --all -- --check`。
  - 当前工作树包含大量既有跨平台和换行修改，本轮仅做定向 rustfmt 检查，避免全局格式化引入无关变更。
- [x] 运行 `git diff --check`，无空白错误。
  - 完成日期：2026-08-29
- [x] WSL Debian 运行 `cargo check --bin simadmin`。
  - 完成日期：2026-08-29；存在既有 dead-code warnings，无编译错误。
- [x] 重新运行定向测试。
  - `register::tests`：23 passed; 0 failed。
  - `volte::sip::tests`：36 passed; 0 failed。
  - `volte::live::tests`：53 passed; 0 failed。
  - `carrier_catalog::v7::tests`：12 passed; 0 failed。
  - `volte::channel::tests`：4 passed; 0 failed。
  - `core::access::tests`：3 passed; 0 failed。
  - `vowifi::channel::tests`：6 passed; 0 failed; 1 ignored。
  - 完成日期：2026-08-29
- [x] 已排查 `vowifi::channel::tests` 在 WSL 中的长时间运行问题。
  - oversized fragmented UDP loopback 测试已明确标记 ignored；原因是 WSL2 loopback 在超过 MTU 时不投递该数据报，不是代码死锁。
  - 完成日期：2026-08-29
- [x] 运行完整后端测试：`cargo test --bin simadmin -- --test-threads=1`。
  - 结果：1335 passed; 0 failed; 3 ignored。
  - 完成日期：2026-08-29
- [ ] 在 Debian/aarch64 目标环境完成编译或 CI 构建。
- [ ] Windows 原生编译仍受已知 `libc::IFF_UP` 平台问题影响；本项不作为本轮 IMS 修复的失败依据。
## 5. 尚未实现：真实接入网络上下文

当前 CNI/PANI 某些路径仍使用静态模板或全零 cell-id。这只能用于兼容性探测，不能作为最终通用实现。

### 5.1 统一数据模型

- [ ] 定义可跨 VoLTE、VoWiFi、未来 VoNR 使用的运行时接入上下文。
  - 当前注册 PLMN（MCC/MNC）。
  - 归属 PLMN。
  - RAT：E-UTRAN FDD/TDD、NR NSA、NR SA、WLAN 等。
  - LTE ECGI/ECI/TAC 或 NR NCGI/NCI/TAC。
  - cell information age 与采集时间。
  - roaming 状态及数据来源可信度。
- [ ] 明确“数据未知”语义。
  - 未知时应 omit、只发 access type，还是使用 profile 静态值，必须由 profile 策略决定。
  - 禁止把未知 cell-id 静默伪装成真实的全零 cell-id。
- [ ] 为每条线路隔离运行时接入上下文，禁止使用进程级共享可变全局状态。

建议落点：

- `backend/src/connectivity/core/access_network.rs`
- `backend/src/connectivity/modems/ims/access_network.rs`

这两个文件当前为新增文件，应先审阅现有内容，再扩展，避免创建重复模型。

### 5.2 从 modem 获取真实数据

- [ ] 从 ModemManager/QMI 读取 serving system、RAT、注册 PLMN、TAC 和 cell identity。
- [ ] 明确 QCM410 当前固件能稳定提供哪些字段，以及字段刷新事件。
- [ ] 将数据按 `line_id` 注入 IMS access leg，而不是由 SIP builder 主动访问全局 modem。
- [ ] 增加过期和切换处理：小区切换、漫游切换、LTE/NR 切换后刷新上下文。
- [ ] 对无权限、无服务、字段缺失和 modem 重启提供可观察错误，而不是 panic。

### 5.3 动态生成 PANI/CNI

- [ ] 用真实运行时上下文生成 VoLTE `P-Access-Network-Info`。
- [ ] 用真实运行时上下文生成 VoLTE `Cellular-Network-Info`。
- [ ] 审核 VoWiFi PANI/CNI 语义：WLAN 接入类型与蜂窝辅助信息必须分别建模，不能复用 LTE 字符串。
- [ ] 对 home/visited network、FDD/TDD、LTE/NR 分别增加单元测试。
- [ ] 为 profile 提供明确策略：`omit`、`static`、`dynamic-if-known`、`required-dynamic`。
- [ ] 删除或降级当前 `{PLMN}0000000` 占位生成逻辑；保留时必须明确标为 compatibility fallback 并由 profile 显式启用。

## 6. 尚未完成：VoWiFi refresh 对等性

共享 REGISTER driver 已提供事务过滤；本轮已完成 VoWiFi refresh 的代码层 requeue 和失败阈值修复，但仍未完成真实 ePDG/IKE 网络验收，也没有直接复制 VoLTE 的候选阶梯。

- [x] 已定位 VoWiFi registration refresh 的全部入口和定时器。
  - 完成日期：2026-08-29
- [x] refresh 等待期间的 NOTIFY、MESSAGE、OPTIONS、INVITE 会被 requeue，并在 refresh 后交还会话循环处理。
  - 完成日期：2026-08-29
  - 自动测试：MWI NOTIFY、inbound MESSAGE/INVITE 及 refresh 相关 live tests 均已覆盖。
- [x] requeue 帧在 TCP/UDP channel 转换、IPsec 重协商和 `into_parts` 后不丢失。
  - 完成日期：2026-08-29
  - 自动测试：`vowifi::channel::tests`（6 passed; 0 failed; 1 ignored）。
- [ ] 评估是否需要与 VoLTE 相同的有限候选阶梯。
  - 不要直接复制 VoLTE 顺序；需要根据 VoWiFi profile 字段定义安全候选，并确保已认证或已建立安全关联后不回退到违反运营商策略的不安全形态。
- [x] 已增加 VoWiFi refresh 与 MWI NOTIFY 同时到达的回归测试。
  - 完成日期：2026-08-29
- [x] 已增加 refresh 与 inbound MESSAGE/INVITE 同时到达的回归测试。
  - 完成日期：2026-08-29
- [x] 已增加 refresh 连续失败后仅在合理阈值触发 ePDG/IKE 重建的测试。
  - 完成日期：2026-08-29
- [ ] 在真实 ePDG/IKE、运营商 WLAN 和终端环境完成网络验收。
## 7. 尚未完成：运营商配置与 `omit` 全链路

catalog v7 投影已支持若干 REGISTER 字段的字符串 `omit`，但还需要确认从数据库到最终 SIP 报文的端到端一致性。

- [ ] 列出所有支持三态的字段及其最终行为，形成唯一 schema 文档。
- [ ] 检查 profile override、profile record、profile store 和配置导入导出是否保留 `omit`，不能在中途变为缺失值。
- [ ] 检查以下字段的端到端测试：
  - `security_agreement`
  - `include_pani_initial`
  - `include_pani_authenticated`
  - `include_route_header`
  - `include_p_preferred_identity`
  - `always_add_sip_instance`
  - `enable_cellular_network_info`
  - `require_sec_agree_headers`
  - `proxy_require_sec_agree_headers`
- [ ] 明确 `security_client_mechanisms` 与 `sec_agree_mode=disabled` 的关系。
  - 数据可以保留以便往返序列化，但 live layer 不得发送 Security-Client offer。
- [ ] 增加最终 SIP 报文断言，不只验证中间 `RegisterPolicyRecord`。
- [ ] 为错误类型值增加验证和诊断，避免无提示回落到默认行为。

## 8. 尚未实现：VoNR/5G IMS 实际链路

当前工程只有 LTE/NR 通用数据模型基础，不代表已经支持 VoNR。

- [ ] 实现 NR SA IMS PDU session/bearer 建立。
- [ ] 实现 5GS QoS flow、QFI 和语音媒体承载映射。
- [ ] 从 modem/provider 获取 NR serving cell、NCI、TAC、注册域与 IMS capability。
- [ ] 生成符合 NR 接入的 PANI/CNI，禁止仍标记为 E-UTRAN。
- [ ] 处理 EPS fallback、RAT handover 和 registration continuity。
- [ ] 实现并验证 VoNR capability 探测；不能仅凭设备支持 5G 就报告 VoNR ready。
- [ ] 增加 NR SA 注册、主叫、被叫、双向 RTP、DTMF、BYE、短信及回落场景测试。
- [ ] 在支持 NR IMS 的真实硬件和运营商网络上验收。

相关现有说明：`docs/ue-isolation-migration.md` 的 5G IMS 章节。

## 9. 实机验收矩阵

### 9.1 REGISTER 与 refresh

- [ ] 初始 REGISTER：无认证直返 2xx。
- [ ] 401/407 AKA challenge 后成功。
- [ ] 423 Min-Expires 协商后成功。
- [ ] 421/494 sec-agree 升级后成功。
- [ ] refresh 等待期间收到 MWI NOTIFY，不掉注册且 NOTIFY 最终被处理。
- [ ] refresh 等待期间收到 SMS MESSAGE，不掉注册且短信最终被处理。
- [ ] refresh 等待期间收到 INVITE，不掉注册且被叫正常振铃。
- [ ] refresh 第一候选失败、降级候选成功时，不重建 bearer/ePDG。
- [ ] 所有有限候选失败后才重建 access，并有明确诊断。

### 9.2 Profile 兼容性

- [ ] 完整 MMTEL feature tags + 动态 PANI/CNI。
- [ ] SMS-only Contact。
- [ ] 显式 omit PANI。
- [ ] 显式 omit CNI。
- [ ] sec-agree auto、required、disabled/omit。
- [ ] 有 Route 与无 Route。
- [ ] roaming visited network identity。
- [ ] 至少两个不同运营商 profile，避免为单一 Maxis 行为过拟合。

### 9.3 业务能力

- [ ] VoLTE 主叫。
- [ ] VoLTE 被叫，不进入语音信箱。
- [ ] 双向 RTP 与静音恢复。
- [ ] SMS over IMS 收发。
- [ ] MWI SUBSCRIBE/NOTIFY。
- [ ] ViLTE capability 与视频媒体协商（若当前版本宣称支持）。
- [ ] VoWiFi 主叫、被叫、切换和 refresh。

## 10. 可观察性与诊断

- [ ] REGISTER 日志加入事务键的脱敏摘要：Call-ID hash、CSeq、候选 label。
- [ ] 记录跳过帧的类型、方法、Call-ID 是否匹配，但禁止输出认证 nonce、Authorization 和完整用户身份。
- [ ] 区分以下失败：传输超时、无关帧洪泛、provisional 洪泛、状态行非法、事务键不匹配、认证拒绝、策略拒绝。
- [ ] 记录实际 PANI/CNI 来源：dynamic、static profile、compatibility fallback 或 omitted。
- [ ] refresh 降级成功时记录被移除的头，不记录完整 SIP 报文中的敏感字段。
- [ ] 为每条线路分别统计 refresh 成功率和 access rebuild 次数。

## 11. 当前工作树注意事项

截至本轮复核（2026-08-29），工作树包含大量未提交修改和以下未跟踪内容：

- `.ci-dl/`
- `.ci-patch.txt`
- `7733.zip`
- `r6-arm64.tar.gz`
- `r6/`
- `r7/`
- `simadmin_1.1.7-beta8.tar.gz`

这些内容可能是参考包、构建产物或逆向分析材料。后续开发不得默认删除、移动、格式化或加入提交。

当前修改跨越 IMS、platform config、数据库和文档。提交前应按逻辑拆分，至少建议：

1. REGISTER transaction filtering 与 channel requeue。
2. refresh 候选阶梯。
3. PANI/CNI runtime policy。
4. catalog v7 `omit` 投影。
5. 配置存储/迁移与文档。

## 12. 每项完成记录模板

复制以下模板到已完成项下：

```text
完成日期：YYYY-MM-DD
提交：<commit hash>
实现摘要：<一句话>
自动测试：<命令和结果>
实机环境：<设备/固件/运营商/接入方式>
实机结果：<REGISTER/主叫/被叫/RTP/SMS/MWI>
遗留风险：<没有则写“无已知风险”>
```

## 13. 完成定义

本计划只有同时满足下列条件才可整体标记完成：

- [ ] 第 4 至第 10 节的适用项目全部完成或明确标记为延期并说明原因。
- [ ] 定向测试和完整后端测试通过。
- [ ] Debian/aarch64 目标构建通过。
- [ ] 至少两个运营商 profile 完成 REGISTER refresh、主叫和被叫实机验收。
- [ ] 不再使用未标记的伪造 cell-id。
- [ ] `omit` 从数据库到最终 SIP 报文有端到端测试。
- [ ] 文档、配置 schema 和诊断字段与实现一致。


## 14. 第二阶段：每线路 VoLTE Profile 编排

### 14.1 目标与边界

本阶段把 VoLTE 的“连接重试”从重复使用同一个 carrier profile，改造成每条物理基带/读卡器独立保存的三个 profile 候选槽位。该策略属于 `LineProfileConfig`，不做成全局设置；SIM 更换后仍由该物理线路保留自己的编排规则。

必须区分两层尝试：

1. **Profile 候选尝试**：完整切换用户数据库、下载 catalog、3GPP 派生 profile；这是本阶段新增的外层三次尝试。
2. **单个 Profile 内部的 REGISTER 状态机**：401/407 AKA、423 Min-Expires、421/494 sec-agree，以及有限的 REGISTER 报文形态兼容候选；这些仍在单个 profile 尝试内部执行，不能与外层 profile 候选混为一谈。

### 14.2 持久化模型

- [x] 在 `LineProfileConfig` 新增 `volte_profile_selection`，并保证旧配置反序列化时自动得到默认值。
- [x] 每条线路固定保存三个有序候选槽位，默认顺序为：
  1. `database`：用户自行创建或导入到主应用数据库的 profile。
  2. `carrier_catalog`：下载并只读使用的 carrier catalog profile。
  3. `derived`：根据 SIM IMSI/Home PLMN 自动派生的 3GPP profile。
- [x] 每个候选保存 `{ source, profile_id? }`，不能只保存裸 `profile_id`，避免用户数据库和下载 catalog 同名时产生歧义。
- [x] `profile_id = null` 表示在该来源内按 IMSI/Home PLMN 自动选择；非空表示严格选择该来源中的指定 profile。
- [x] `derived` 候选禁止设置 `profile_id`。
- [x] 三个槽位允许用户重新排序；同一来源可重复出现，以支持测试不同显式 profile。
- [x] 旧 `SimOverride.ims_volte.profile_id` 保留兼容：线路候选未显式指定 ID 时，只在与旧 profile 实际来源一致的槽位中作为来源内 pin 使用；线路显式 ID 优先。
- [x] ConfigManager 增加 get/set/validate，并用测试证明线路 A 的修改不会影响线路 B 或独立读卡器线路。

### 14.3 来源解析和兜底规则

- [x] `database` 自动模式只搜索用户数据库，不得被下载 catalog 的同名 profile 覆盖。
- [x] `carrier_catalog` 自动模式只搜索下载 catalog，并要求 LTE/EPC 投影可用。
- [x] 显式指定 profile 时严格限制来源，用户数据库和下载 catalog 的同名 ID 必须可分别选择。
- [x] `derived` 始终根据当前 SIM 的 Home PLMN 派生，不从数据库读取。
- [x] 当 `database` 或 `carrier_catalog` 槽位没有可用 profile（来源不存在、无匹配项、显式项已删除或无 LTE 投影）时，该槽位改用派生 profile，并在运行时记录原请求来源和兜底原因。
- [x] 按用户要求保留三个逻辑槽位：若用户数据库和下载 catalog 都不存在，则三个槽位都可解析为同一个派生 profile，并实际执行三次；不得因去重而静默缩减为一次。
- [x] 派生也失败时返回可诊断错误，错误中只包含来源、槽位和脱敏后的 PLMN/IMSI 前缀，不输出完整 IMSI、AKA 数据或鉴权头。

### 14.4 运行时与重试行为

- [x] VoLTE 自动恢复批次最多按三个候选槽位执行，默认依次为用户数据库、下载 catalog、派生配置，不再无差别重复同一个 profile。
- [x] 每次候选切换前清理上一 profile 的 bearer/profile lease、P-CSCF、REGISTER 对话和临时安全关联，禁止跨 profile 复用 nonce、Route、Security-Verify 或 Call-ID。
- [x] Baseband-wedged 等不安全重试错误仍立即中止整个批次，不能为了尝试下一个 profile 继续冲击基带。
- [x] 运行时状态增加：当前候选索引、请求来源、请求 profile ID、实际 profile ID、实际来源、是否派生兜底、失败原因和每槽位结果。
- [x] 成功后保留实际生效 profile 的 ID/来源，下一次全新恢复批次仍从用户配置的第一个槽位开始。
- [x] 手动“重试”启动一个新的三槽位批次；REGISTER refresh 失败后的 access rebuild 也启动新的三槽位批次。
- [x] 非恢复路径（例如发送短信时发现 VoLTE 未连接）必须复用同一候选编排入口，不能绕过策略只尝试旧的自动匹配。

### 14.5 后端 API

- [x] 新增 `GET /api/volte/lines/{line_id}/profile-selection`，返回线路策略、可选 profile 列表和当前运行时解析结果。
- [x] 新增 `PUT /api/volte/lines/{line_id}/profile-selection`，保存三个有序候选并做来源/ID 校验。
- [x] 可选 profile 列表必须按来源分开返回，并至少包含 `profile_id`、PLMN、品牌/名称、来源、LTE ready 状态和 catalog release/source。
- [x] API 不得把下载 catalog profile 写入用户数据库；选择只保存引用。
- [x] 保存正在连接或已注册线路的策略时，先持久化，再安全断开旧会话并启动新批次；离线线路只保存配置。
- [x] API 错误码覆盖：无效线路、候选数错误、derived 带 ID、来源不支持、显式 profile 不存在、显式 profile 无 LTE 投影。

### 14.6 前端

- [x] 在 IMS 页面每条线路的“VoLTE / IMS 注册”旁新增“配置”按钮，交互位置和 VoWiFi、Trunk 保持一致。
- [x] 新增 VoLTE Profile 配置对话框，清楚显示“这是每条基带/读卡器独立设置，不是全局设置”。
- [x] 对话框提供三个可排序槽位；每槽位可选择来源，并在用户数据库/下载 catalog 来源下选择“自动匹配”或指定 profile。
- [x] 数据库或 catalog 不存在时显示“本槽位将使用派生配置兜底”，但仍允许保存。
- [x] 显示旧 SIM profile pin 的兼容状态，以及线路显式选择对它的覆盖关系。
- [x] 显示当前实际生效 profile、来源、候选槽位、兜底原因和最近三个候选结果。
- [x] 保存后刷新该线路，不影响其他线路卡片的状态和表单。

### 14.7 自动测试

- [x] 配置默认值和旧 JSON 迁移测试。
- [x] 每线路/读卡器隔离和持久化测试。
- [x] 三槽位顺序、重复来源、显式 ID 和 derived 禁止 ID 的验证测试。
- [x] 用户数据库与 catalog 同名 ID 的来源隔离测试。
- [x] 用户数据库自动匹配、catalog 自动匹配、显式匹配和派生匹配测试。
- [x] 用户数据库缺失、catalog 缺失、两者都缺失时的逐槽位派生兜底测试；两者都缺失时断言实际保留三次派生尝试。
- [x] 第一槽位失败第二槽位成功、前两槽位失败第三槽位成功、三槽位全部失败测试。
- [x] Baseband-wedged 批次立即中止。
  - 测试：`api::handlers::tests::volte_profile_batch_aborts_on_baseband_wedge_or_generation_change`。
- [x] 候选切换的逻辑运行时状态清理。
  - 测试：`connectivity::modems::ims::volte::live::tests::profile_switch_aborts_listener_and_releases_line_scoped_registration`、`connectivity::modems::ims::volte::runtime::tests::profile_switch_clears_session_ownership_without_cancelling_the_batch`。
- [x] 手动 retry 重置到槽位 1。
  - 测试：`connectivity::modems::ims::volte::runtime::tests::a_new_manual_retry_batch_restarts_at_slot_one`。
- [ ] 真实 bearer/QMI endpoint、AT CID、xfrm/IPsec、P-CSCF reporting 和 IMS profile lease 释放的集成测试。
- [x] API helper 的 GET/PUT payload、校验、离线保存、在线重启和跨线路隔离。
  - 测试：`api::handlers::tests::volte_profile_selection_*`。
- [ ] 完整 HTTP/AppState API 集成测试。
- [x] 前端 TypeScript 类型检查、lint 和 production build。
  - 完成日期：2026-08-29；已执行 `pnpm type-check`、`pnpm lint`、`pnpm build:full`。
- [ ] 浏览器级 VoLTE Profile 对话框交互测试。

**本轮自动验证记录（2026-08-29）：**

- WSL Debian：`cargo test --bin simadmin -- --test-threads=1` 通过，结果为 `1335 passed; 0 failed; 3 ignored`。
- WSL Debian：`cargo check --bin simadmin` 通过；仅有既有 dead-code warnings。
- 定向测试：`register::tests` 23、`volte::sip::tests` 36、`volte::live::tests` 53、`carrier_catalog::v7::tests` 12、`volte::channel::tests` 4、`core::access::tests` 3 均通过；`vowifi::channel::tests` 6 passed、1 ignored。
- `git diff --check` 通过。
- 前端：`pnpm type-check`、`pnpm lint`、`pnpm build:full` 均通过。
- 仍缺：完整 HTTP/AppState 集成测试、真实 bearer/QMI/xfrm/P-CSCF/profile lease 资源释放集成测试、浏览器 E2E，以及第 14.8 节真实设备/网络验收。

### 14.8 实机验收

- [ ] 用户数据库 profile 成功注册并完成主叫、被叫、双向 RTP、短信和 refresh。
- [ ] 下载 catalog profile 成功注册并完成同样业务矩阵。
- [ ] 前两来源不可用时派生 profile 兜底注册。
- [ ] 切换候选时抓包确认 Call-ID、CSeq 对话、安全关联和 P-CSCF 未跨 profile 污染。
- [ ] 两条不同基带线路配置不同顺序并同时运行，确认互不影响。
- [ ] 独立读卡器绑定线路保存/恢复自己的顺序，确认不读取其他基带的设置。


## 15. VoWiFi 自定义 DNS 恢复

- [x] 确认后端自定义 DNS 数据模型、API 和 ePDG 解析逻辑仍然存在，缺失点主要是 VoWiFi 配置对话框的前端控件。
- [x] 在 VoWiFi 线路配置页面恢复 SIM/线路级“自定义 ePDG DNS”输入项。
- [x] 支持填写多个 DNS 服务器，每行一个，并按用户填写顺序依次尝试。
- [x] 保存到该 SIM/线路的 `ims_vowifi.dns` 覆写字段，不影响其他线路。
- [x] 清空输入后保存为 `null`，回退到 carrier profile DNS，再回退到系统/内置 DNS。
- [x] 前端增加 IPv4、IPv6 及带端口 DNS 输入校验。
- [x] 后端增加 `ims_vowifi.dns` 校验，拒绝非法地址和端口 0。
- [x] 后端定向测试：`effective_profile::tests` 16 passed。
- [x] 后端 VoWiFi live 定向测试：69 passed。
- [x] 前端 `pnpm type-check`、`pnpm lint` 和 `pnpm build:full` 通过（2026-08-29）。
- [x] 410 实机确认 `ims_vowifi.dns` 可持久化，配置 `1.1.1.1`、`8.8.8.8` 后 ePDG 解析阶段达到 `epdg_ready`（2026-08-29）。
- [x] 410 实机故障注入确认运行时确实使用线路级 DNS：配置不可达的 `192.0.2.1` 后日志记录该地址并返回 ePDG DNS 超时（2026-08-29）。
- [ ] 真实设备/运营商网络完整验收：指定 DNS 已能解析 ePDG，但当前 `50212` 派生 profile 在后续 IKE/IMS REGISTER 阶段收到网络 `400/421`，尚未完成 IKEv2、Child SA、ESP 和 VoWiFi 注册闭环。
  - 说明：本项不能仅因 `epdg_ready` 勾选；需要可用 carrier profile 或运营商允许的真实 VoWiFi 参数后重新验收。
