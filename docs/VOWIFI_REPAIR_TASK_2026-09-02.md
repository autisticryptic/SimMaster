# VoWiFi/VoLTE 修复任务跟踪

更新时间：2026-09-02（北京时间）
适用设备：QCM410 `192.168.100.13`  相关线路：Vodafone Germany（PLMN `26202`）

本文档是本轮修复工作的唯一任务清单。后续代码、测试、提交、部署和
兜底配置审查都按照本文档推进；只有得到对应的代码、构建、设备日志或
文档证据后，才将项目勾选为完成。

## 约束与测试口径

- [x] Vodafone Germany 只使用 UDP：IKE/500、NAT-T/4500、IMS REGISTER
  和 REGISTER refresh 均为 SIP over UDP。
- [x] 不用 TCP 测试 Vodafone Germany，也不把
  `sms.receiver_transport = tcp` 解释为 IMS REGISTER 使用 TCP；该字段仅
  属于 SMS over IMS 接收通道。
- [x] 不使用 compact REGISTER；完整的能力、身份和安全协商头必须保留，
  大报文通过 IP/ESP 外层软件分片与重组处理。
- [x] 默认承载地址族顺序为 `ipv4v6 -> ipv6 -> ipv4`；只有网络明确返回
  单栈要求时才立即切换到对应单栈。
- [x] 项目继续只使用 UE network namespace，不回退到宿主网络命名空间。
- [x] 禁止本地编译；构建必须由 GitHub Actions 生成 amd64/arm64 产物，
  410 设备只部署 ARM64 产物。

## 目标清单

### 1. 410 设备上的真实续订验证

- [x] 确认 Vodafone Germany VoWiFi 初始 REGISTER 返回 200 OK。
- [x] 确认 Vodafone Germany VoLTE 初始 REGISTER 返回 200 OK。
- [x] 确认 VoLTE REGISTER refresh 在协商租期内成功。
  证据：2026-09-02 09:35:06 CST，`register_phase="refresh"`，
  `expires_seconds=3136`。
- [x] 确认 VoWiFi REGISTER refresh 复用现有 ePDG/IKE/ESP/TUN，且使用 UDP
  并返回成功。
  证据：2026-09-02 09:35:36–09:35:38 CST，
  `transport="udp"`、`reused_access=true`、
  `SIP/2.0/UDP`，成功后 `expires_seconds=3355`。
- [x] 确认续订期间没有 `No such device`、
  `ims_ue_udp_socket_creation_failed` 或因 TCP 选择导致的失败。
- [x] 观察下一次刷新或重连周期，确认本次成功不是一次性偶然结果。
  证据：10:24:39–10:24:41 完成新一轮完整注册，租期 3041 秒；
  11:12:17–11:12:20 自动 refresh 复用访问链路并返回 200 OK，
  `transport=udp`、`reused_access=true`、新租期 3162 秒。

## Device evidence update (2026-09-02 10:30 CST)

At 10:24:39-10:24:41 CST the current Vodafone Germany profile completed a
second full VoWiFi registration using ipv4v6, UDP/500, UDP/4500, and the
existing outer software fragmentation path. The REGISTER candidates received
494, then 403, then an AKA challenge; the authenticated full REGISTER returned
200 OK with a 3041-second lease. At 11:12:17-11:12:20 CST the automatic refresh
reused the access path, remained on UDP, returned 200 OK, and negotiated a new
3162-second lease. This establishes repeatable initial registration and
refresh behavior rather than a one-off success.

At 12:01 CST the live loop detected a stale access cache with the TUN missing
while the UE veth and worker were still present. It discarded that cache,
rebuilt IKE on UDP/500, NAT-T on UDP/4500, the Child SA and TUN, then completed
the full fragmented REGISTER ladder with 200 OK at 12:01:48 CST
(`expires_seconds=3086`).

At approximately 12:08 CST an ESP outbound send failed at the same time that
ModemManager replaced modem object 70 with object 71 and briefly reported
`couldn't find modem`. This evidence points to modem re-enumeration rather than
an operator rejection or a regression in UDP fragmentation. The live loop
automatically rebuilt the access path at 12:09 and again completed
494 -> 403 -> AKA -> 200 OK. This validates the observed modem-reenumeration
recovery, but does not claim that every possible socket or namespace failure
has been exercised.

### 2. 续订和失效恢复逻辑审查

- [x] 使用网络返回的 Contact `expires`/`Expires` 租期计算刷新时间，按
  11/12 租期提前刷新。
- [x] VoWiFi refresh 保留 ePDG/IKE/Child-SA/ESP/TUN，并按连续失败阈值
  决定是否重建访问链路。
- [x] VoLTE refresh 在 live loop 中执行，并在失败时记录简短的
  `registration_loss` 原因。
- [x] OPTIONS 未响应只作为保活诊断，不单独宣告 REGISTER 失效。
- [x] 对实际发生的 TUN 过期和基带重枚举路径完成设备级恢复验证：缺失 TUN
  会清理 stale cache 并重建 IKE/ESP/TUN；modem 70→71 重枚举后的 ESP 发送
  失败会触发下一轮自动重建并重新注册成功。
- [ ] 继续保留连续 refresh 失败和 UE socket 单独消失这两类尚未被实机触发
  的异常路径，后续出现时再验证；不得把当前证据扩大为所有异常均已覆盖。

### 3. 无数据库 VoWiFi 派生/兜底配置

- [x] 复核 PLMN、IMS 域名、ePDG Operator Identifier FQDN 和 EAP-AKA NAI
  realm 的标准格式化入口。
- [x] 复核派生 profile 默认 `ipv4v6 -> ipv6 -> ipv4`、UDP、IKE/ESP
  proposal、EAP-AKA、PANI/CNI、MMTEL 和完整 REGISTER。
- [x] 复核显式 ePDG IP、显式 DNS、最后使用系统 resolver 的优先顺序。
- [x] 复核 494/403 等响应驱动的有限 REGISTER 变体回退，以及 423/
  `Min-Expires` 处理。
- [x] 对照 TS 23.003、TS 24.302、TS 24.229、TS 33.203、RFC 7296、
  RFC 3261、RFC 3329 和 RFC 5626，列出当前实现缺口。
- [x] 仅补齐可以从标准和运行时上下文安全推导的值；不得从 PLMN 猜测
  私有 P-CSCF、私有 ePDG、运营商 DNS、AKA identity template 或非标准
  IPsec 参数。
- [ ] 为新增兜底行为补充单元测试，并验证不污染数据库搜索结果。
  测试代码已经补齐，且数据库浏览继续保留 IMS-only 自定义记录；GitHub
  Actions 已通过 `cargo test --no-run` 编译全部测试代码，但尚未实际执行
  profile-store 测试，因此本项继续保持未完成。

代码审查与逐项依据见 `VOWIFI_REGISTRATION_FALLBACK_AUDIT.md`。本轮已新增
数据库/目录/派生共用解析器，以及 IMS-only 数据库记录不得进入 VoWiFi
运行时解析器的回归测试；同时修复了 `ims_vowifi.profile_id` 已读入快照但
未传给候选解析器的问题。因本任务禁止本地编译，测试执行状态等待 CI。

## Standards review status

| Reference | Checked behavior | Result |
|---|---|---|
| TS 23.003 | PLMN/MNC padding, IMS home domain, operator ePDG FQDN, EAP-AKA realm | Implemented in the standard naming helpers; private MCC 999 is rejected. |
| TS 24.302 | ePDG discovery/selection and address-family fallback | Explicit IP/UICC/visited/profile/derived ordering is present; `ipv4v6 -> ipv6 -> ipv4` is retained unless the network explicitly requests one family. |
| TS 24.229 | SIP REGISTER, PANI/CNI, MMTEL, Contact lease | Full REGISTER is retained, PANI/CNI are policy-gated, and Contact expiry drives refresh. |
| TS 33.203 | IMS IPsec security negotiation | Security-Client/Server/Verify and protected REGISTER transition are implemented; no private security tuple is invented. |
| RFC 7296 | IKEv2 exchanges, retransmission, NAT-T | UDP/500 and UDP/4500 behavior is implemented; Child SA/TUN state is reused for refresh. |
| RFC 3261 | REGISTER expiry and 423/Min-Expires | 423 retry parses Min-Expires and is bounded; refresh uses the negotiated lease. |
| RFC 3329 | Security agreement headers | 494-driven negotiation and authenticated Security-Verify handling are implemented. |
| RFC 5626 | instance-id/reg-id and flow behavior | `+sip.instance` is advertised where configured; `reg-id` is not guessed because outbound-flow support and operator policy are not derivable from PLMN. |

The implementation already has standard PLMN/ePDG/IMS naming, the documented
address-family order, UDP/full REGISTER behavior, Security-Client/Server/
Verify negotiation, Expires/423 handling, and outer IP/ESP fragmentation.
No private P-CSCF, ePDG, DNS, AKA identity template, or non-standard IPsec
value is safe to invent from PLMN alone. The remaining work is a documented,
source-backed item-by-item comparison and tests for any safe gap found.

### 4. 构建、提交、部署和最终同步

- [x] 修改后执行 `cargo fmt --check` 与 `git diff --check`；禁止本地
  `cargo build/check/test`。
- [x] 提交并推送 GitHub，等待 Actions 成功生成 amd64/arm64。
  证据：commit `872d4cb`；GitHub Actions run `33591379418` 的测试代码编译、
  ARM64、AMD64 和 Release 发布 job 均为 success。
- [x] 下载 ARM64 产物部署到 410，确认服务启动、版本和 commit 正确。
  证据：Release `v1.1.4` 包内 `meta.json` 为 commit `872d4cb`、
  `aarch64-unknown-linux-musl`，设备二进制 MD5
  `4448ce75527275523c14d5bb8e7a53d8` 与发布包一致；服务于
  2026-09-02 12:51:00 CST 启动。
- [ ] 重新执行 Vodafone Germany UDP-only 初始注册与 refresh 验证。
  初始注册已经完成：VoWiFi 于 12:54:15 CST 返回 200 OK，租期 3220 秒；
  VoLTE 在部署触发的 modem 72→73→74 重枚举稳定后，于 12:54:57 CST 返回
  200 OK，租期 3344 秒。两者的本轮自动 refresh 尚待约 13:43–13:46 CST
  实机日志确认。
- [ ] 将最终源码、文档和 `.github/workflows` 覆盖同步到
  `D:\Program\AI\FileSystem\SimAdmin-Enhance`；不复制 `.git`、`target`、
  `.ci-dl`、`.ci-patch.txt` 或设备数据库。
- [ ] 在同步目录和主仓库分别记录最终测试结果与未完成项。

### 5. 主任务完成后的界面状态修复

以下两项排在本轮 VoWiFi/VoLTE 解析、CI 构建和 410 部署复测之后处理，
不得中途打断当前主任务：

- [ ] 修复 IMS 与 Trunk 页面的 IMS 续期次数显示。当前现象是后端已发生
  refresh 后页面不会实时更新，必须刷新浏览器才显示；同时计数语义或
  初始值有误，例如设备从当天 00 点后已经保持 VoLTE 注册，但 09 点后仍
  显示为“第一次续期”。需要核对后端计数生命周期、服务重启边界、状态
  API/WebSocket 推送以及前端响应式更新，确保次数与日志中的成功 refresh
  一致。
  代码已完成：成功 refresh 会原子写入按线路持久化统计（最多 256 条），
  新 runtime 会恢复计数；用户明确关闭 VoLTE 或切换 eSIM 时清零。线路
  状态轮询已改成单飞并合并后续刷新，不再因慢请求持续互相作废。等待 CI
  和 410 上的重启恢复、实时更新与关闭清零验证后勾选。
- [ ] 修复切换 eSIM profile 后概述页手机号仍显示旧 profile 号码的问题。
  需要在 ICCID/profile 切换完成后使号码来源及相关缓存失效并重新读取，
  同时避免旧 Skinny/上一 profile 的 MSISDN 残留到新卡状态。
  代码已完成：切换前后均清理线路 IMS 观察号码和物理 QMI slot 身份缓存，
  同时删除旧/目标 ICCID 的非手动号码缓存并在基带恢复后刷新线路身份；
  概述页 SIM/设备信息每 10 秒单飞刷新。手动号码仍按设计保留。等待 CI 与
  410 多 profile 切换验证后勾选。

## 当前结论

Vodafone Germany 的初始 VoWiFi/VoLTE 注册、连续 UDP refresh、缺失 TUN
重建以及 modem 重枚举后的自动恢复均已取得成功日志。VoLTE 的 OPTIONS
未响应属于诊断性保活现象，不能作为 TCP 或正式 REGISTER 失败依据。规范
对照和无数据库兜底审查已完成；数据库、目录和派生候选现在共用
access-aware 解析器，IMS-only 数据库记录不会再进入 VoWiFi live matcher。
下一步是由 GitHub Actions 执行新增测试和双架构构建，再部署新的 ARM64
产物复测。续期计数持久化/实时刷新和 eSIM 号码缓存失效的实现已经完成，
尚待本轮 CI 与 410 实机验证；尚未实机触发的异常路径继续保留为未完成项。
