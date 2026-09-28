# IMS 只读诊断与日志投影

规范脚本：[`scripts/ims-readonly-evidence.sh`](../scripts/ims-readonly-evidence.sh)。
当前任务/设备状态见 [接手入口](HANDOFF.md)，本文只维护工具契约，不另存一份进度清单。

## 1. 覆盖的证据

- 初始 REGISTER 请求、AKA challenge、认证请求、实际发送端口。
- 未完整收到响应、终止 SIP 响应、注册成功、原通道 refresh 和重试。
- restore 失败、P-CSCF 观察/轮换、承载结束、UE worker 生命周期。
- `/opt/simadmin/meta.json` 安装信息与 `/proc/<MainPID>/exe` 实际运行哈希分开输出。
- 在日志采样前后复核 PID/start ticks，不能把安装 metadata 自动当作运行版本。

只投影明确事件中的类型化白名单：地址族、端口、布尔存在标志、认证轮数、SIP 状态码、
CSeq、已知错误码、租期/重试时间。不回显原始日志行或自由文本错误详情。

真实 `%error` 可能是未加引号的 `code:detail`，并含嵌入引号或假字段。脚本只取白名单 code，
把后续整段当作不透明详情，禁止把其中的 `sip_status=` / `request_cseq=` 当成元数据。
未知错误为 `unlisted_error_redacted`，不能猜测原码或用它判断具体根因。

## 2. 不输出与不执行的内容

- 不输出号码、SIM 身份、PDU、IP/P-CSCF 地址、SIP URI、Cookie、RAND/AUTN/CK/IK/RES 等认证材料。
- 不执行 AT/QMI、API 写入、承载创建、启停服务、owner 切换、NV/USB 写入或重启。
- 不读取应用配置/数据库，不更改密码，不自动 retry/reconnect，不伪造硬件恢复。
- 不需要设备安装 Python/jq/JSON::PP；依赖 POSIX sh、Perl、systemctl/journalctl 与常规 coreutils。

## 3. 使用

在已有授权入口恢复后，可通过 SSH exec 的 `sh -s` 从 stdin 发送脚本，不必在设备安装文件。
设备操作者审阅后也可本地执行：

```sh
sh scripts/ims-readonly-evidence.sh
```

只对已安全取得的日志做离线投影，不查询服务/设备：

```sh
sh scripts/ims-readonly-evidence.sh --filter-log < private-journal.txt
```

原始 journal 保持私有，不提交或直接粘贴。当前本机 SSH 入口在 `.local/active/ims/`，
位置与凭据规则见 `.local/README.md`；旧 `.codex-*` 生成器/部署脚本在本地 archive，不能重放。

## 4. 输出上限与解释

- 仅读本次开机最近 6000 行 journal，最多输出 120 条匹配记录。
- 单行超过 8192 字节整行丢弃并计数，不让超长无换行输入耗尽内存。
- journal 读取失败显式 `journal_read_failed=true` 并以非零退出，不伪装成空白成功。
- `matched=0` 可能表示时间窗口、日志级别、格式或版本不匹配，**不证明从未尝试注册**。
- `oversized>0` 说明证据不完整。
- `running_process_stable_during_sample=true` 只证明采样期间主进程没变。
  `journal_scope=current_boot_all_service_processes` 可能包含当前开机内旧程序或子进程日志，
  不能都归到当前哈希，也不能未经 API/时间/实例核对就归到某张 SIM。

## 5. 与线路状态联合定位

另行经授权应用登录，只 GET 线路列表/详情和 `/api/modem/backend`，核对真实 line ID、
SIM 作用域、backend、runtime 的 `phase/stage/last_error`、尝试记录及下一重试时间。

- `radio` / `bearer`：核对附着、IP 族、owner 和未解决 receipt；不写 NV 或猜 CID。
- `pcscf`：核对本次承载与实际发现来源；不能直接复用旧卡地址/APN。
- `REGISTER_NO_COMPLETE_RESPONSE`：结合认证轮数区分初始/认证后传输失败，不当作收到 403。
- `REGISTER_CHALLENGE`：只证明走到挑战处理，不证明 AKA 已通过；后续失败需核对实际错误码。
- `REGISTER_TERMINAL_RESPONSE`：保留真实 SIP 状态和事务阶段，但状态码本身仍不证明根因。
- `REGISTER_SUCCESS`：初始与 refresh 分开；重新建会话不是原通道续期，注册也不是业务验收。

## 6. 回归

`.github/scripts/test_ims_readonly_evidence.py` 的 28 项测试覆盖实际源日志字段、
`aka_empty_uri_first` 授权枚举、SIP/无响应、引号与未引用 error 注入、类型与重复字段、
上限/超长行、journal 失败以及模拟采样期间 PID 变化。

本地只做 Python/语法检查，Rust 编译与回归仍交给 Actions；最新实际执行结果见接手记录。

## 7. SIM-06：两套成功参考的生产链对照（2026-09-27）

### 7.1 证据与关键纠正

用户确认 **beta8** 与 **`simadmin-volte-main.zip` 构建的成品** 都能注册同一张 SIM-06 中国电信卡。
这是两套成品，不混称为同一个实现；也不以本项目暂时失败推断该卡不支持 IMS。
本节形成时设备仍离线，是代码/历史静态资料对照，**不是现场根因或修复验收**；
2026-09-27/28 上线后的新证据见第 9 节。

- **P**：本项目已发布 `1.1.5 / 16998ae`，下列 P 行号固定于该源码。
- **R**：上级目录源码 ZIP，SHA-256 为
  `7bee591d9f292ba5129114eb4c221aa77b7fac66d04bd553434155fb82d98750`。
  本地位于 `.local/reference-volte-20260926/simadmin-volte-main/`，关键文件已逐字节与 ZIP 比对。
- **B**：归档 [beta8 对照](archive/2026-09/IMS_DERIVATION_BETA8_COMPARISON_2026-09-17.md)
  的样本 `1.1.7-beta8 / 930365d`，二进制 SHA-256
  `210c35b11f54dd240a83e90dd08d5e8a8f4f2cea227ce3a0503a9ced4140f9b7`。
  本轮 IDA `get_metadata` 返回连接拒绝（10061），**没有新的 IDA 验证**；B 的结论只引已有 B01–B09/§8.1。

**R 的真正注册路径是 Python daemon，而不是 ZIP 中的 Rust 逆向辅助模块：**

```text
backend/src/volte_manager.rs:22,388,428–430,538–558
  -> /opt/simadmin/volte_register.py
  -> volte_register.py:49–58 启动 ims_runtime.py
  -> ims_runtime.py:122–174 manager / rebuild
  -> ims_bearer.py:110–189 建立承载和取得地址/P-CSCF
  -> ims_protocol.py:133–282 Session / register
```

R 的 `scripts/pack-ota.sh:13–18,211–216` 与 `install_latest.sh:1202–1207` 也将这些 Python 文件
放入运行包。Rust `volte/identity.rs` 确有 `460 -> 2 位 MNC` 和长 APN helper，但不能把这些
辅助实现当成 daemon 的执行证据。旧参考审计把两条路径混在一起的部分以本节为准。

### 7.2 有实际调用依据的差异

| 项目 | R 实际执行 | P / beta8 对照与意义 |
|---|---|---|
| 身份 / MNC | `ims_protocol.py:140–148` 直接读 MM **SIM** 的 `operator-code` 并生成 home domain；此链没有 EF_AD 或 MCC 460 猜测调用 | P `cellular_ims/live.rs:6694–6818` 也接受匹配 IMSI 的 SIM operator，额外支持确认 home 的驻网信息及 UIM/AT EF_AD。B 的 B01/B02 确有末级国家猜测；它**不是 R/B 成功共同依赖** |
| EF_AD / AID | R `ims_protocol.py:148–152` 读 card status 找 USIM AID，缺失即失败；不调用 Rust 的内置 AID fallback。`ims_runtime.py:389` 的 CRSM 是短信相关读取，不是 MNC 的 EF_AD | P `identity.rs:73–118` 有前后 IMSI 核验的 CRSM EF_AD，`live.rs:6796–6810` 有 12 秒边界；QMI UIM 支持同卡 EF_AD。不能因 R 未读 EF_AD 就断言 P 缺来源 |
| IMS APN / profile | R `ims_bearer.py:30–37,125–146` 查找 **APN=ims 且 IPv6** 的 QMI profile，缺失时新建 IPv6 profile，然后以同一 profile/WDS client 启动 | P `profiles.rs:1148,1180–1250` 标准派生为短 APN `ims`，`live.rs:2099–2183` 使用 effective APN。P `pcscf.rs:243–299` 按 APN/优先 CID 选现存 AT context，并非 R 的 exact IPv6 QMI profile 选择；B §8.1 B/C 强调 family 与 lease 关联 |
| 承载 / 普通数据副作用 | R `ims_bearer.py:76–97,122–146` 按 data_enabled 选 wwan0/wwan1，直管 WDS、BAM-DMUX 绑定及 family=6；`ims_runtime.py:147–174,205–226` 在数据关闭且判断不活跃时临时 `simple-connect apn=3gnet`，结束时 `simple-disconnect` | P MM 默认路径由 `qcm410/ims_bearer.rs:160–210` 的 `PrimaryImsSession` 管理 MM bearer，保留 UE 隔离和数据意图；没有 R 的硬编码 3gnet 激活。B 有 MM/direct WDS 两条路径，但缺少 SIM-06 成功时走哪条的证据。R helper 存在也不证明成功时分支一定执行 |
| P-CSCF 时序 | R `ims_bearer.py:138–169` 激活前开 reporting；QMI settings 取 IPv6 地址/网关，P-CSCF 实际取自 CGCONTRDP。为空则**再次写 reporting**、等 5 秒、重读一次 | P `live.rs:2146–2183` **已经在建立承载前开启 reporting**；随后 retained provider/同址 AT 补充。`pcscf.rs:476–630` 已有 6 轮、间隔 1 秒、总预算 12 秒的只读等待，但不在该循环重复写 reporting。B §8.1 E 的六轮读取已被覆盖，不是 P 完全没等 PCO |
| REGISTER / 安全 | R `ims_protocol.py:176–192,247–275`：IPv6、明文 5060 初始空 AKA，固定声明 Require/Proxy-Require，`hmac-md5-96/null`，401 后 UIM AKA、四向 XFRM、受保护认证 REGISTER；固定端口 42001/42002 | P `live.rs:783–815,2816–2883` 的 offer 取当前 profile 第一项；标准派生 `profiles.rs:1247–1250` 为 `hmac-sha-1-96/aes-cbc`，Require 根据策略/响应升级。P `ipsec.rs:432–437` **已支持 MD5/null**；非 strict profile 可接受合法 server offer（`live.rs:1583–1605`）。因此是**实际声明形状**差异，不是“P 完全不支持 MD5” |
| Contact / 续期 | R Contact 声明 smsip，不含 P 的 MMTEL/audio/+sip.instance；PANI 是固定接入类型。R `ims_runtime.py:122–170` 到期前调用 rebuild/初始注册，通常可复用存活 WDS，但会更换 SIP/XFRM 会话 | P 标准派生声明 MMTEL/audio/instance、使用已知动态 PANI，并有原会话自然 refresh。B 另有较宽的 plain fallback 和定时重入。成功初始注册不证明参考的语音能力或续期更完整 |

P 路径前缀：`cellular_ims/` = `backend/src/connectivity/modems/ims/cellular_ims/`；
`profiles.rs` = `backend/src/connectivity/modems/ims/vowifi/profiles.rs`；
`qcm410/` = `backend/src/hardware/devices/qcm410/`。R 路径相对 ZIP 顶层目录。

### 7.3 现场只读判别顺序，不按无数据的“概率”硬排根因

1. **先确认是否在 bearer / P-CSCF 之前失败。** 比较真实 backend、当前/请求 profile 的 PDP family、
   实际授予地址族、MM 与 WDS owner，以及默认数据是否本来活跃；如果尚未发送 REGISTER，
   不能把安全算法或 Contact 写成根因。不要将 AT CID、MM profile-id、WDS client ID 混为同一编号。
2. **到 P-CSCF 时**，检查 reporting 是否成功、实际来源与重读次数。P 已有启用和等待，
   真正未等价的是 R 的再次写 reporting、exact-family profile 和额外数据面激活。
   需要任何重挂载/写 reporting/临时承载对照时，另开有归属与清理证明的授权窗口；
   不能在“只读采证”中执行 `simple-connect` 或 `simple-disconnect`。
3. **确实进入 SIP 后**，先读取安全的算法名、Require/Proxy-Require、MMTEL/instance/PANI 存在标志、
   auth_rounds 与真实 SIP 状态。不得收集 nonce/密钥。R/B 的 MD5/null 与 P 的默认 offer 不同值得对照，
   但不因此全局降低安全配置、停掉 MMTEL、固定端口或加入任意错误后的明文回退。
4. **身份仅在来源真的缺失时检查。** P 有可靠 SIM operator 时不会依赖 MCC 猜测。
   `identity.rs:147–150` 实际返回 `CARRIER_PROFILE_MISSING` + detail `home_plmn_mnc_length_ambiguous`，
   `live.rs:2002–2004` 将该类错误映射到 `carrier_profile`；profile store 还可能给出其他 detail。
   不能沿用此前口述的四个不存在的 `HOME_PLMN_*` 独立码或“必定停在 starting”的结论。

**本次没有改注册算法、没有运行参考程序或访问离线设备。**
下一轮若启用 IDA，应先复核上面的 beta8 样本哈希，再查看 B04/B08 的具体参数和返回路径；
不能仅凭旧函数地址、Rust 辅助模块中的说明或“两套参考都能注册”就重放设备写操作。

## 8. 后继候选修补：Security-Server 列表（2026-09-27）

本节描述 **16998ae 发布之后**的代码修补；不将静态问题等同于 SIM-06 的实机根因。

按 [RFC 3329 §2.2 / §3.4](https://www.rfc-editor.org/rfc/rfc3329.html)，服务端可以通过多行或
逗号分隔的列表给出安全候选；选择可用机制与回传 `Security-Verify` 是不同职责，后者必须保留
服务器给出的完整列表、顺序及参数。旧实现只选择整行，分号拆解可能跨逗号混读参数；多行时又只
保存所选一行，且没有在选择阶段排除本端不支持的算法。

修补范围：

- 新的纯解析模块 `cellular_ims/security_agreement.rs` 支持多行/逗号形式、带引号的逗号/分号及转义，
  先隔离每项候选再取参数，禁止借用别的候选补齐 SPI/端口。
- 只从本端支持且符合既有 strict profile 限制的机制中选择；按有效显式 q 值择优，
  显式 q 冲突/重复参数/损坏引号/控制字符拒绝。为兼容既有无 q 的单候选，缺省优先值为 1，
  同为隐式优先级时保留 wire 顺序；不声称所有无 q 列表都是严格 RFC 3329 格式。
- 最高可用候选缺必需绑定时失败，不偷偷退到低优先级机制。算法与端口/SPI来自同一候选；
  `Security-Verify` 回传完整有序原列表，包括未选择的机制。
- 初始认证和后续 protected refresh 使用同一选择器；新 XFRM 使用选定的类型化算法，
  不再把完整 verify 列表塞回单机制算法解析器。旧会话的 rollover 核对也能处理列表。
- 设定 16 KiB 头部总量、16 个候选、每项 32 个参数的边界；无硬件、网络或进程副作用。

**不改变默认算法或客户端请求形状**：本轮仍保留 `Security-Client` 取 profile 第一项的现状，
没有自动追加 MD5/null，也没有改变 MMTEL、MM 默认、明文回退或身份派生。
配置多项客户端机制是否应全部声明，需要独立设计，不把这次服务端修补包装为全部协商能力完成。

新增 15 项纯 Rust 解析测试和 2 项 live 接线测试；已有 protected refresh 测试改为使用与其
MD5/null 测试 SA 一致的严格测试 profile，并验证认证请求完整回传列表、旧通道回滚/保留。
两套 Actions 已加入独立模块测试过滤器，`dfda6cd` 的新增与既有回归实际执行通过：
[Validate `36291639402`](https://github.com/autisticryptic/SimMaster/actions/runs/36291639402)、
[Build `36291639395`](https://github.com/autisticryptic/SimMaster/actions/runs/36291639395) 均 success，
包含前端、Rust 回归和 amd64/arm64 musl 构建/打包；Publish Release 按 push 门禁 skipped。
本地定向格式、diff 与 172 项 Python 守卫通过，未进行本地 Rust 编译。

**代码/CI 已完成，但未实机验证，也未覆盖已发布 `v1.1.5 / 16998ae` 的资产。**
如 SIM-06 实际停在承载或 P-CSCF 阶段，本修补并不能解释它的失败；仍先取得现场分层证据。

## 9. SIM-06 现场与 CID 修复（2026-09-27/28）

### 已实际核验的失败阶段

用户在历史会话末尾确认设备上线，本次恢复后已通过现有 Cookie、SSH 固定公钥和应用登录。
只读核验实际运行及安装文件 SHA-256 均为
`afcc9ecbe4331dd3cfa31b392920bad1cf096fb0f10f35f864490804790d6588`，metadata 为
`1.1.4-beta3 / 2129282`，不是发布的 `1.1.5 / 16998ae`。采样主进程 PID 454，未重启。
`/api/modem/backend` 显示 `modemmanager`；唯一现存线路 `line-50ad…`，SIM operator `46011`。

- 线路启用 IMS、普通数据开关关闭，配置地址族顺序为 `ipv4v6 → ipv6 → ipv4`。
- 三个 profile 槽位（derived/catalog/database）实际均回落到 `derived_3gpp_lte_46011`。
- 三次日志均先报 `cellular_ims_preferred_profile_occupied`，没有 `REPORTING_ENABLED`。
  随后创建 APN-only 的 MM IMS bearer，请求双栈、实际仅授予 IPv6（有网关及两个 DNS）。
- retained-MM P-CSCF 观察返回 `qca410_primary_mm_pcscf:context_pcscf_absent`；最终
  `stage=pcscf`、`recovery_state=exhausted`、`retry_attempt=3`、`registered=false`。
  08:55–08:59 UTC 三次尝试日志属于 PID 454，并与 API 时间和 bearer path 交叉核对；尚未进入 REGISTER。
- 只读 AT 查询确认：CID 1=`IPV4V6/ctlte`（活动），CID 2=`IPV4V6/ctwap`（不活动），
  **没有独立 `ims` 定义**。reporting 表 CID 1–16 均为 `0,0,0`。
  2026-09-28 再查 `AT+CGDCONT=?` 明确报告 IP、IPV6、IPV4V6 均支持 CID 1–16。

这说明当前代码在首选 CID 被其他 APN 占用时没有准备专用 IMS profile，也就跳过了该 profile
的 reporting 启用。它是已证实的前置失败路径；**尚不能证明创建 profile 后一定下发 P-CSCF或注册成功**。
不能把“未发现 P-CSCF”进一步推断为运营商在所有情况下都不下发，也不能据此调整 AKA/安全算法。

### 修复与验证边界

`8df57a9` 初版允许在首选 CID 被占用时选空闲 CID；其 Build `36325894646` 和 Validate
`36325894682` 实际全绿。本次部署前进一步要求：

1. 已有匹配 IMS 定义原样复用；没有时读取 `AT+CGDCONT=?`，按所请求 PDP 类型解析支持集合。
2. 从支持集合选未定义 CID，保留首选优先，否则选最低空闲 CID；**从不新建 CID 1**，不覆盖
   `ctlte/ctwap`、其他定义或空 APN 占位；全满、能力缺失/损坏均失败，不猜支持范围。
3. 创建前确认不活动，重新读取完整定义行并对照快照；出现并发变化则不写。
4. 只写一个新的 `CGDCONT` 定义，读回确认 CID/APN/PDP 类型。不发 `CGACT=1/0`、不改默认附着，
   不执行普通数据激活；后续 bearer 仍由原 MM 路径管理，并在建立前开启对应 CID 的 reporting。
5. 新定义保留供复用；写入未确认不自动重复或删除，不套用旧 prefetch 的覆盖/恢复逻辑。

新增纯解析和异步假 IO 回归覆盖能力按族匹配、能力范围无效、保留 attach/占位定义、并发占用、
活动 CID、原样复用、未确认写入及 AT 参数注入。Rust 编译与执行仍只在 Actions。补强版 `e0ade97` 的 Build `36368975285`、Validate
`36368975330` 全部通过，7 项新 Rust 回归实际执行，本地 173 项 Python 检查通过。

### 首次覆盖结果与后继适配

`1.1.5 / e0ade97` 已于 2026-09-28 02:28 UTC 部署，主进程 PID 288352；
实际运行 SHA-256=`badb454b965a4e68d3e69b7c7ddf1dbaf035fe4eebaea62527ef5189894a6c33`。
MM PID 410、secondary PID 283 均未变化。现场仍在创建 profile 前被
`cellular_ims_profile_definition_ambiguous` 拒绝，没有覆盖原 CID 1/2，也没有注册成功。

原因是生产 `control::at_command` 通过 **mmcli stdout** 返回 `response: '…'` 外壳，
而新增严格定义/能力校验只接受纯 AT payload；D-Bus 的原始回复与 CLI 输出不能混为一谈。
已补充只剥离完整已知外壳的归一化，并增加完整创建流程的 CLI 回包测试、残缺外壳/ERROR 回包拒绝测试。
后继 `02dfdc5` 的 Build `36370931907` 与 Validate `36370931916` 均实际成功，9 项新增
Rust 回归执行通过，本地 173 项 Python 检查通过。已于 02:55 UTC 直接覆盖部署，无新增备份。
实际二进制 SHA-256=`8041d6860fd5866245517bdf450183a643e82e307df792632711b186358e4784`。

新程序成功创建 `CID 3 / IPV4V6 / ims`，并执行 `REPORTING_ENABLED cid=3`；原 CID 1/2 未改。
但三次尝试仍为 `context_pcscf_absent`，没有 REGISTER。只读查询进一步发现：

- MM Bearer Properties 报 `profile-id=3`、APN=ims、已连接、授予 IPv6。
- AT `CGACT?` 仅 CID 1 活动；`CGCONTRDP=1` 实际 APN 为 ims，地址与 MM 同 /64、不同 IID，
  仅 7 个字段（无 P-CSCF 列）。CID 3 未活动、`CGPADDR=3` 未给有效地址。
- QMI profile 列表明确 profile 1/2/3 的 PDP context number 分别为 1/2/3，不是简单静态编号偏移。
- 本机 MM 为 1.18.4；其 profile pin 优先加载 profile 的 PDP 类型，不保证调用者的 `ip-type` 覆盖它。

### 撤销 IPv6-only 对照，保留原兜底

仅对本次程序新建、确认已释放的 CID 3 做过一次 `IPV4V6 → IPV6` 单变量对照，未修改源码的
默认策略或持久化 `cellular_ims_ip_families`，也未修改原 `ctlte/ctwap`。该对照**仍未获得 P-CSCF**。
用户指出这种固定 profile 类型会干扰原 `ipv4v6 → ipv6 → ipv4` 兜底，意见成立：MM pin 会优先
使用 profile 类型，所以“配置顺序没改”不等于真实兜底完全不受影响。

已于 2026-09-28 03:23 UTC 撤销这个实验，读回确认 CID 3 为 **IPV4V6/ims**，CID 1/2 完全保留，
配置顺序仍为 `ipv4v6 → ipv6 → ipv4`。未回滚程序，仍运行 `1.1.5 / 02dfdc5`，PID 307733，
MM/secondary 的 PID 不变。证据：`.local/evidence/sim06/deploy-02dfdc5/address-family-restored.json`。
后续不得把固定 IPv6 当作最终修复，也不能据此宣布其他族/配置已验收；继续围绕实际 MM bearer、
活动 PDP 与 PCO 来源排查，保留并正确实现原地址族策略。

### 2026-09-28 已授权 MM 窗口与首次注册成功

用户后续批准：在核验实际承载 IMS 的上下文后打开 reporting，并经 MM 进行一次重新附着，
用于验证可推广的兜底机制，不固定 IPv6、不使用 direct WDS/AT 后端。

本次只有一个 MM 自有 IMS bearer，实际 APN=ims/profile-id=3，只有一个活动 context（CID 1），
其 `CGCONTRDP` 实际 APN=ims、IPv6 /64 与 MM grant 一致、没有 P-CSCF、reporting 全关。
释放应用自有资源后，仅在这个已确认的 context 上开启 reporting，通过原 MM unique owner
执行一次 Disable→Low Power→Enable，没有变更 ctlte/ctwap/ims 定义或 Initial EPS。
脚本等待条件误用了 `state>=9`（MM REGISTERED 实为 8，9 为 DISCONNECTING），因此错误地
报告等待超时；后继自动恢复的回归必须覆盖状态 8，不用这个错误结果推断重新驻网失败。

**现场结果**（程序仍为 `1.1.5 / 02dfdc5`，主 PID 455798）：

- 06:29:36 UTC 开始从 retained MM bearer 成功关联两个 P-CSCF；此时实际活动 context 已为 CID 3。
- 第一个派生槽位在认证阶段收到终止 401；第二槽位实际也回落 derived，终止 403。
- 第三槽位仍是 `derived_3gpp_lte_46011`，先遇到无响应，按已有候选轮换后，
  **06:32:29 UTC 初始注册成功，registered=true，registration_mode=ipsec**。
- 配置仍是 `ipv4v6 → ipv6 → ipv4`，CID 3 仍为 IPV4V6，普通数据关闭，MM daemon 与 secondary
  PID 未变。不据此声称短信、通话或自然续期已通过；也不把实际源 derived 误写成数据库 profile。
- 有效证据：`deploy-02dfdc5/mm-reattach-once.json`（含脚本等待错误）、
  `deploy-02dfdc5/observed-20260928T063308Z.json`（运行哈希、API、SIP元数据、PDP、服务状态）。

这证实本次组合维护后 P-CSCF 和注册恢复，不能单凭这一次对照证明是某条写命令独自起效，
更不能声称所有 modem/运营商都需要重新附着。通用实现保持以下边界：

- 原 P-CSCF 发现、DNS、配置来源和地址族兜底先执行；只对最终的 derived/P-CSCF 缺失评估恢复。
- 使用原 MM 自有 lease、unique owner、SIM、唯一 bearer、当前 grant 与实际协商 APN 的多重核验。
  非同 CID 的实际上下文只生成恢复提示，不把相同前缀当作可借用别的 context 的 P-CSCF 证明。
- 没有通话、数据或同 modem 其他线路冲突，确认原 lease 完全释放、定义/EPS 未改后，才执行一次。
- `/run/simadmin/mm-pcscf-recovery/` 的每 MM owner/设备/SIM 预算在写入之前独占持久化，
  正常重试、profile 轮换或应用重启不能清零；失败不会再触发 Disable/Low Power 循环。
- MM REGISTERED 状态 8 即可重新运行原 profile/address-family batch，不等待先出现应用 bearer。
  取消和错误路径仍受原 owner/用户意图控制，不偷偷重启服务、改初始 EPS 或固定地址族。

代码和自动路径的实际 CI/部署验收需另外记录，不能把上述手动维护窗口冒充新代码已执行。

### 2026-09-28 续接核验与恢复安全补强

- `7896e05` 的 Build `36389546245`、Validate `36389546247` 均 success；官方 artifact SHA-256、
  双架构包版本/commit/ELF 及 7 项新增 Rust 恢复/准入回归已实际核验，旧 Release 没有覆盖。
- 07:25 UTC 只读复查仍为 `02dfdc5` / PID 455798：registered=true、IPsec，
  `last_register_refresh_at=07:22:30 UTC`、`register_refresh_count=1`。这证明旧部署在维护后完成
  一次自然续期，不冒充新候选部署证据。无通话、管理走 wlan0、地址族顺序未变。
- 部署前审查发现：旧恢复步骤的补偿 Enable 也依赖 IMS generation/data/VoWiFi 条件，
  Disable 后普通配置取消可能阻止无线电归位。补强将继续恢复与 radio 补偿分离：
  只有本次已经尝试 Disable 后才补偿一次；原 owner/endpoint/SIM 和明确 airplane/off 意图仍可阻止 Enable，
  不因普通 IMS 取消而把无线电留在低功耗状态，也不再执行第二次 Disable。
- 真实 Disable 前再次经原 MM unique owner 查询 ListCalls，未知/有通话不执行破坏性步骤；
  Low Power 写入前及等待循环内重查意图/身份。失败明确显示 degraded/modem/exhausted，
  不因清理重置而停留在 starting。新增硬件无关取消/状态等待/诊断回归；本地 179 项 Python 检查通过。
- 后继 `dd8ba1f` 新构建与部署已核验（见下节）；自动重新附着的实机故障注入分支仍未验收。
  当前已启用 reporting 的已注册设备不应为强制覆盖测试而关回上报、
  清除预算或反复重新附着。预算仅在同一系统启动内的 MM owner/device/SIM 范围持久有效，
  不宣称跨系统重启或 MM owner 变化后永久耗尽。



### 2026-09-28 09:49 UTC 注册与自然续期收尾

本轮重新读取两份历史会话后，以当前设备和 GitHub 状态纠正交接滞后：

- `dd8ba1f` 的 Build `36392766359`、Validate `36392766357` 均 success。
  已重新校验本地双架构 artifact digest、包/二进制 SHA-256 及日志中的 21 项新增 Rust 回归；
  其中 12 项为 MM 恢复/取消/状态等待回归。本地 179 项 Python 测试通过。
- 设备安装 metadata 为 `1.1.5 / dd8ba1f`；当前 PID 511308 的运行二进制 SHA-256
  `3ae12007b981bbeb0efca220365b8701f391ab16009346fa7ad457f072900e4d` 与 ARM64 制品一致。
  采样前后进程稳定，MM PID 410、secondary PID 283。本轮未重复部署、未重新附着。
- 同线路 API 确认 `registered=true`、`registration_mode=ipsec`、有效源为 derived，
  profile 为 `derived_3gpp_lte_46011`。07:54:27 UTC 初始注册成功；08:44:28、09:34:30 UTC
  的 `REGISTER_SUCCESS register_phase=refresh` 与 API 续期时间/计数 2 对应。
  三个配置槽的有效源均为 derived，第三槽轮换 P-CSCF 后成功，不归功于数据库专用配置。
- 原 `ipv4v6 → ipv6 → ipv4` 顺序保留。实际授予 IPv6，不是固定 IPv6 配置。
- **注册与自然续期已验证；新自动重新附着分支的实机故障注入未验证。** 当前 P-CSCF 已存在，
  不能将普通自动注册的 `recovery_source=automatic` 当作该分支触发证据。不为测试而关闭上报、
  清除预算或破坏健康会话；取消/恢复边界目前依赖硬件无关回归，不扩称全设备验收。

本轮证据位于 `.local/session-review/` 的 `verified-runtime.json`、`connection-result.txt`、
`current-ci.json`、`python-tests.log`；既有制品与回归证据位于 `deploy-dd8ba1f/`。
本次仅核验既有部署，不覆盖 Release，不将短信、通话、native 或长期业务矩阵勾选完成。

### 部署授权及取证文件

用户已授权提交 GitHub、部署新的 **1.1.5 构建**并做一次 SIM-06 注册验证。已发布 Release
仍为 `v1.1.5 / 16998ae`，新候选使用独立 Actions 制品，不覆盖历史 tag 或 Release。
切换前核验官方 artifact digest、包内 commit/版本/架构、无通话和管理路径；不启停 MM、
不写 NV/USB、不修改 Initial EPS，不恢复旧的测试窗口自动回滚策略。
用户随后明确实验机**直接覆盖，不再保留备份**；首次部署刚生成的备份已删除，后续不再创建备份。
这不包括删除历史诊断、凭据或现有配置/数据库。

本地脱敏证据在 `.local/evidence/sim06/`：`connection-summary.json`、
`runtime-readonly-20260927T123301Z.json`、`pcscf-readonly-20260927T123754Z.json`、
`pdp-readonly-*.json`、`cid-fallback-initial-ci.json`、`cid-initial-artifacts.json`、`cid-initial-jobs.json`。
本地采证辅助脚本只导出类型化元数据；raw journal、API 凭据、SIM 标识及网络地址不公开。
