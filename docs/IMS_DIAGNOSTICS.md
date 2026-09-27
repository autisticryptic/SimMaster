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
本项目设备仍离线，本节是代码/历史静态资料对照，**不是现场根因或修复验收**。

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
