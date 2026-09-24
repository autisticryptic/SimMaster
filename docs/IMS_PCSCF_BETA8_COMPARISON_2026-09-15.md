# SIM-04 P-CSCF 失败与 beta8 二进制对照（2026-09-15）

> 这是有时间边界的开发/实测记录，不是 1.1.5 发布验收。
> 本次仅测试 MM 后端的 SIM-04 IMS 注册；没有混合后端、短信或电话测试。
> **6391732 实测未注册；后续 P-CSCF/DNS 修补不能被回填为该次实测成功。**
> 2026-09-24 更新：T03/T04 的初始成功见 §7；后续 T05 原始日志已核实 7 次自然续期，
> `71513ea` 正式服务已核实 9 次。详见 [续接验收](SIM04_CONTINUATION_2026-09-24.md)，
> 不再把旧短窗口“尚无续期”当作当前故障。

## 1. 参考文件身份

用户提供的参考包：
[simadmin_1.1.7-beta8.tar.gz](https://github.com/lilith-rong/SimAdmin-Enhance/raw/refs/heads/Backup-Vowifi-and-VoLTE/Volte/simadmin_1.1.7-beta8.tar.gz)。
用户报告该版本能够正常注册；本轮没有在设备上运行参考版，也未独立验收它的全卡支持范围。

| 核对项 | 本次读取结果 |
| --- | --- |
| 包内版本 / commit | `1.1.7-beta8` / `930365d` |
| 包内构建时间 / 架构 | `2026-07-27T10:47:10+08:00` / `aarch64-unknown-linux-musl` |
| 本次下载包 SHA256 | `ab943a799421d1759d611be089342ba3427382cd8a0c7a9327b5cd4228854bdd` |
| ELF64 AArch64 二进制 SHA256 | `210c35b11f54dd240a83e90dd08d5e8a8f4f2cea227ce3a0503a9ced4140f9b7` |
| 二进制 MD5（与包内 metadata 一致） | `d0903ceab475bacaf00e7ef45d1403c5` |

这些摘要用于标识本次文件，不是发行者签名。IDA MCP 当前载入文件的 SHA256/MD5 与下载文件一致。
以下地址采用本次 **base=0 的 IDB 地址**；对 `.text` 的指令另用 ELF/FDE 范围与 AArch64
反汇编交叉核对。反编译存在 Rust 编译器拆分辅助函数和不完整类型恢复，因此不把自动变量名当作原源码。

## 2. 沿交叉引用确认的行为

| IDB 函数 / 关键指令 | 确认内容 | 对当前实现的意义 |
| --- | --- | --- |
| `sub_1A8674`，`0x1a90ac`、`0x1aa010`、`0x1aa060`、`0x1aa0e0` | 上层流程可先读当前活动 `AT+CGCONTRDP`，随后按路径调用 profile 准备、MM bearer 或自行持有 WDS 的实现 | 存在不同路径；不能只见到一条字符串就断言全部卡都走同一路径 |
| `sub_19A008`，`0x19a058`、`0x19a088`、`0x19a498` | 读取 PDP 定义/活动情况，维护占用集合并租用未占用 CID；有已租用 CID 时复用它 | 当前 `6391732` 复用已有同 APN 定义，或只使用确认缺失的 preferred CID，策略不同 |
| `sub_1A0A80`，`0x1a0ab0`、`0x1a0ae8`、`0x1a0b44` | 对租用 CID 依次构造 `CGACT=0`、`CGDCONT=<cid>,"<type>","ims"`、`$QCPDPIMSCFGE=<cid>,1,1,1` | beta8 会按尝试的类型准备定义；当前测试保留既有 `IPV4V6` 定义。尚未证明该差异就是 P-CSCF 缺失原因 |
| `sub_19B9F4`，`0x19c00c`、`0x19c02c`、`0x19c22c` | 没有预取地址时，活动上下文查询为 `1..=6`；调用 `sub_3EA970` 等待 1 秒再读。`sub_5E564` 的边界语义已核对 | 可以移植有界只读等待，不需要恢复 AT 激活或重写用户 profile |
| `sub_19D6DC`，`0x19d7d0`、`0x19da0c`、`0x19dbe4` | 分配并保留自己的 WDS client，设置 family，以 `apn=ims,3gpp-profile=<cid>,ip-type=<family>` 建立网络，再读取 current settings | 自有 WDS CID 的查询不等于可以借用 MM 的内部 CID |
| 同函数 `0x19e218`、`0x19e3dc`、`0x19e3f4`、`0x19e518` | 专门查询 WDS `0x002d`，请求 TLV `0x10` 的 mask 为 `0x0c00`，读取返回 TLV `0x23` 的 4-byte 地址列表；IPv6 标志分支跳过这段直接查询 | 这是 IPv4 P-CSCF 列表路径，不是 SIM-04 IPv6 的现成修复 |

QMI 消息号、mask 和 TLV 含义另与 libqmi `1.28.6` 的
[`qmi-service-wds.json`](https://github.com/linux-mobile-broadband/libqmi/blob/1.28.6/data/qmi-service-wds.json) 和
[`qmi-enums-wds.h`](https://github.com/linux-mobile-broadband/libqmi/blob/1.28.6/src/libqmi-glib/qmi-enums-wds.h) 对照。

**不应移植的行为**：扫描或复用别人的 bearer/CID、未经设备能力确认就写新 CID、没有恢复凭据的
profile 重写、宿主 namespace 发送 IMS、默认三位 MNC 猜测、把 DATA6 改成 IMS。
本次确认的准备函数不是“先 `CGACT=1` 再 WDS”的依据；仓库历史 prefetch 路径的注释不能替代这个二进制的控制流。

## 3. SIM-04 实测事实

时间均为 Asia/Shanghai。详细逐卡记录见
[项目交接第 12 节](PROJECT_HANDOFF_2026-09-12.md#12-后续多卡测试记录持续追加)。

- 09:16 只读重连成功：Qualcomm 410、aarch64、`5.15.0-handsomekernel+`，SIM-04 home `45507`；
  后续运行日志显示 serving `46011`。原程序为 `684e2a7`，管理默认路由走 `wlan0`。
- 使用已过 CI 的 **`63917329b0e09a074c6e526997b67e9db4df36c9`**，重新核对 artifact/包/二进制摘要。
  原程序不覆盖；候选用独立目录、克隆数据库和 `127.0.0.1:13000`，明确 `mode=modemmanager`。
- 第一次切换的防残留检查误将同名的 DATA6 initializer 视为主程序，候选未启动即回滚。
  后续只排除已核对的 DATA6 进程，仍拒绝其他残留主程序；没有杀掉该 initializer。
- 第二个窗口 09:44 启动候选，事先设置 25 分钟回滚 timer；原主程序停止，MM/proxy/DATA6 保持运行。
  候选初始 IMS 关闭，VoWiFi、普通数据、Trunk、eSIM 控制、通知和自动化隔离关闭。
- 09:47 仅开启目标线路 IMS；实际依次执行：

| requested | effective | 结果 |
| --- | --- | --- |
| derived | `derived_3gpp_lte_45507` / derived | `volte_runtime_all_pcscf_failed` |
| carrier_catalog | `profile-ct-mo-45507-046a9073cd` / carrier_catalog | 同上 |
| database | derived（`source_unavailable:database`） | 同上 |

- 三次均建立 IPv6 IMS bearer，并使用该线路 UE worker/netns。09:49 快照确认程序配置的精确 IPv6 地址
  有 `nodad`、`noprefixroute`，没有 tentative；同一接口另有 RA 生成的 tentative 地址，不能混为一个地址。
- 实际 `CGCONTRDP` 只返回 IMS CID 的本地 IPv6、网关和空 DNS 字段，没有 P-CSCF 列。
  只读确认 CID 2 活动、APN `ims`、PDP 类型 `IPV4V6`，P-CSCF 上报开关已为 `1,1,1`。
  对同一已持有 CID 有界重发一次上报设置、等待 5 秒再读，仍没有地址；没有另建承载或追加 SIP retry。
- **没有 P-CSCF 候选，因此没有本次路由到 P-CSCF、SIP 请求、AKA 或注册成功证据。**
  不能说已完整修好旧 IPv6 路由失败，也不能把 AT 可见字段为空等同于网络/WDS 一定未下发 PCO。
- 10:01 关闭候选 IMS，确认候选 receipt 消失、接口归还，再停止候选并恢复原服务/恢复 timer。
  原数据库在原主程序停止期间 SHA256 未变；原程序与配置 SHA256 未变。
  11:48 再只读核对：原 `simadmin.service` active，候选 inactive，测试占用已释放，MM/proxy/DATA6 PID 保持，
  默认路由仍为 `wlan0`。原配置 IMS 意图仍为 true，原 `684e2a7` 此时也为 P-CSCF 缺失、未注册。

## 4. 本轮可移植修补与验证

`1cf849f` 实现，`269be65` 补充活动状态歧义拒绝及日志来源表述：

1. 活动 AT P-CSCF 最多 6 轮、轮间 1 秒，总预算 12 秒（包括 IO）。只读 `CGACT?`、`CGDCONT?`、
   精确 CID 的 `CGCONTRDP`；不激活、不重写 profile，不请求其他 APN 的上下文。
   首次可用的活动 IMS CID/定义被固定；后续变化、矛盾/重复 CGACT 行，或接受地址前的复核不一致均拒绝。
2. 新 `pcscf_dns.rs` 验证事务 ID、QR/opcode/TC/rcode、单个问题的 name/type/IN class，名称比较保留标签边界。
   只采用 Answer 中目标名或最多 8 跳 CNAME 链的同类数据；拒绝环、歧义、截断、跨 RDLENGTH、过长名称和无效压缩。
   Authority/Additional/NS glue 不作为 SIP 地址。
3. DNS SRV 保留 target、port、priority、weight；按 priority 稳定排序，同 target 不同 port 不混合。
   端口贯通 `SocketAddr` 到实际 UDP SIP route/session；显式 PCO IP 仍默认 5060，IPsec 协商端口仍由原 sec-agree 路径决定。
   不把 root target、零端口或 TCP SRV 服务改造成 UDP/5060。
4. DNS 仍通过绑定 UE 接口的 worker socket 查询；不新增宿主解析器或静态运营商地址。

`1cf849f` 的 [Validate](https://github.com/autisticryptic/SimMaster/actions/runs/34925540533) 和
[Build-Release](https://github.com/autisticryptic/SimMaster/actions/runs/34925540557) 均 success；
逐 job 核对 Rust 回归、前端、arm64/amd64 构建及 Publish Release skipped。
本地 61 项 Python 规则检查、6 项前端 unit、rustfmt/diff 通过。该提交新增 19 项 Rust 用例，
`269be65` 再增 1 项 CGACT 歧义用例，共新增 **20 项 Rust 回归**。

最终代码 **`269be650066bc73275b6bd58623db13d3eadbf02`** 的
[Validate Beta Refactor](https://github.com/autisticryptic/SimMaster/actions/runs/34926823080) 和
[Build-Release](https://github.com/autisticryptic/SimMaster/actions/runs/34926823070) 均 success。
已逐 job 核对 Rust 回归、前端与 arm64/amd64 构建成功，Publish Release skipped。
两架构 artifact 的 API 摘要如下；本轮没有下载这些新包，摘要不代表包内二进制摘要：

| artifact | ID | API ZIP SHA256 |
| --- | --- | --- |
| pkg-amd64 | `10380435939` | `2c82e8a5a88a70ca0a281ad00b5bd17217f874102286d10a09d8c37a19d62c67` |
| pkg-arm64 | `10380154179` | `09e6e7ab4dca41f1508d762cd4ac77c59ef3ad49d5596a0119f4c773f107dfa4` |

查询时未过期（到期日 2026-09-18 UTC）；未来部署仍需重新查询、下载并校验。
后续新代码**没有部署到设备**，不能借用 `6391732` 的实测作为其硬件验收。

## 5. 尚未解决

- SIM-04 的 P-CSCF 获取与真实注册仍失败。需要在保持单 owner 的条件下确认 AT、MM bearer 与
  WDS 各自可见的字段，再评估设备能力约束、可恢复的精确-family PDP lease；不能先断言 CID/type 差异就是根因。
- 当前受控读取几分钟后仍无 P-CSCF，因而不能声称“多读 6 次”必然解决本次现场问题。
- DNS 改动不会凭空提供缺失的 IMS DNS；本轮设备失败没有进入 DNS 查询路径。
- 仍不是完整跨来源 P-CSCF 重试策略、强制 override 优先级或配置预览/runtime 来源统一。
  CNAME-only 应答不追加追问；完整 SRV 权重调度、TCP/TLS SIP 支持和配置 URI 自定义端口语义仍需单独推进。
- 混合 owner、Native IMS 实机闭环、SIM-01/02 新版本回归、自然续期与业务验收没有因此完成。

本机原始证据、认证材料、备份和候选脚本保持私密，不随本文提交。

## 6. 2026-09-19 深度 beta8 补证与实现边界

本节补充同一 SHA256=`210c35b11f54dd240a83e90dd08d5e8a8f4f2cea227ce3a0503a9ced4140f9b7` 的 IDA MCP 复核。结论与综合对照文档一致：beta8 的跨运营商成功率来自多层 fallback 和承载路径覆盖，不是单一派生域名。

### 6.1 已确认的优点

- `sub_19014C`：MM/CIMI、SIM/EF_AD、home PLMN/MNC 长度、IMS 域和身份派生。
- `sub_19A008`：读取 PDP 定义/活动状态，维护 CID 占用并按尝试租用 context。
- `sub_1A0A80`：profile 准备格式至少包含 `CGACT=0`、按 family 的 `CGDCONT` 和 `QCPDPIMSCFGE=1,1,1`。
- `sub_196634`：MM 路径携带 `profile-id`、`apn=ims`、`ip-type`、`allow-roaming`。
- `sub_19D6DC`：独立 WDS 路径、自有 client、按 family 建立 IMS WDS，以及 IPv4 WDS P-CSCF 查询。
- `sub_19B9F4`：活动 P-CSCF 有界读取，随后进入路由和注册 runtime。
- `sub_18D548` / `sub_197CC8`：空 AKA、423/401/407、UIM AKA/AUTS、IPsec/plain 注册分支。

### 6.2 不应直接移植的路径

当前证据没有证明 beta8 的所有 MM路径都执行临时 `CGACT=1` 预取；仓库中旧 beta2 helper 的临时激活又存在 QCA410 firmware/DHCP context 释放竞态、完整 profile 恢复不精确、reporting 原值未保存和取消时无补偿清理等问题。因此 `prefetch_pcscf_from_ims_profile` 未接入 5094ac1 的生产 live 路径。

同样没有移植 beta8 的 direct WDS owner、宽泛 IPsec→plain fallback、XFRM flush 或宿主网络 fallback。当前 MM 主线继续保持一个 bearer owner 和 UE namespace 隔离。

### 6.3 当前实现与下一项

5094ac1 已覆盖：MM typed IP4/IP6、实际授予 family、profile pin 保留、retained owner/profile/IP 前后复核、IPv6 同 `/64` 不同 IID 的窄关联、DNS/SRV 严格校验以及取消/清理保护。

剩余真正的代码缺口是 MM 内可恢复的 exact-family profile lease：必须保存完整 `CGDCONT`/profile 定义、CID 存在性、原始 P-CSCF reporting 状态、MM owner/bearer、取消/崩溃恢复和清理后的再读验证。它不能通过普通 IMS retry 自动执行，也不能用 beta8 的自有 WDS 查询替代。

## 7 SIM-04 实机结论更新（2026-09-20 / 2026-09-21）

上文「SIM-04 仍停在 P-CSCF、没有 SIP/AKA 证据」的描述已被后续实机结果取代，保留在此仅作演进记录。

真正缺失的变量不是 beta8 的临时 AT 预取，而是 **attach/reporting 时序**：先启用 P-CSCF reporting，再执行一次受控 `Disable → Low Power → Enable` 重新附着，网络才在同一 CID 2 上把 `CGCONTRDP` 由 7 字段变为 9 字段并下发 2 个 P-CSCF 候选。在此条件下：

- 2026-09-20 T03（`0feaa40`）首次取得 SIM-04 真实初始 IMS 注册；
- 2026-09-21 T04（`70dfe3d`，最终分支）在同一时序下复现注册，确认成功不依赖已被取代的候选。

两轮均为 derived 首槽 `derived_3gpp_lte_45507`、`profile_candidate_index=1`、MM 保留 bearer、IPv6/`wwan0`、P-CSCF `source=mm_owned_at_sole_pinned_ipv6_prefix` / `cid=Some(2)` / `pcscf_count=2`；REGISTER 收到 challenge 后以 `registration_mode=udp` 完成 `standard_3gpp_conservative` 初始注册，`expires_seconds=3600`、`service_route_count=1`、`associated_uri_count=2`、`contact_binding_count=1`、`voice_service=registrar_accepted`。

当时尚未取得的验收项：自然续期（两轮 `register_refresh_count` 均为 0）、通话与短信。
自然续期后来已在 T05 / `71513ea` 取得，见本文顶部的后续记录；通话和短信仍单独待测。

结论修正：beta8 对照的价值在于确认 profile/CID/family/P-CSCF 的分层与有界读取机制，而不是证明需要临时 `CGACT=1` 预取；该预取依旧未接入生产路径。上文关于 `0feaa40` 阻止 forced-family 重试并返回 `profile_pin_family_conflict` 的记述也已作废：`70dfe3d` 判定那是回归风险（几乎所有选中的 IMS profile 都带 `profile_id`，会误关闭已验证的 IPv4 强制单栈路径），已恢复网络强制单栈重试，改为用 `attempted_single` 仅跳过字面重复尝试，并移除 `pinned_profile_forced_family_error` / `profile_pin_family_conflict`。

本节仍不构成 beta8 实机验收；上述是当前项目在 MM 默认后端下的注册结论。