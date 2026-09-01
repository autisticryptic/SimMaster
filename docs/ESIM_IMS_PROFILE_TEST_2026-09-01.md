# eSIM IMS Profile 实机测试报告

日期：2026-09-01  
设备：QCM410，`192.168.100.13`  运行版本：SimAdmin `1.1.4`  代码：`881da2a`  
GitHub Actions：ARM64、AMD64 与测试编译均成功。测试期间设备 fatal 计数：开始 `0`，结束 `0`。

## 测试范围

逐一切换设备上的 11 张 eSIM，观察 VoLTE 蜂窝注册流程。失败项记录 SimAdmin 最后阶段和运营商返回的诊断；运营商能力、漫游协议或网络侧拒绝不作为 SimAdmin 缺陷处理。

| eSIM | 漫游网络 | VoLTE | 观察结果 |
| --- | --- | --- | --- |
| `+7 7070257688 哈萨克斯坦`（Tele2） | MY MAXIS | 失败 | bearer 返回 `[3gpp] option-unsubscribed`，表示 IMS/VoLTE 未订阅 |
| `+64 2040668688 新西兰`（Skinny） | 502/153 | 成功 | `registered=True`；这是本次 UIM/逻辑通道修复后的正向验证 |
| `+42 0796609668 瑞士`（Swisscom） | MY MAXIS | 失败 | 蜂窝网络未注册，未进入可用 IMS bearer |
| `+234 7062657685 尼日利亚`（MTN） | MY MAXIS | 失败 | bearer/蜂窝阶段在观察窗内超时 |
| `+372 59717223 爱沙尼亚` | MY MAXIS | 失败 | 所有 P-CSCF 尝试失败（`volte_runtime_all_pcscf_failed`） |
| `+995 551106339 格鲁吉亚`（Magticom） | MY MAXIS | 失败 | 蜂窝网络未注册 |
| `+31 0683485100 荷兰`（Simyo/KPN） | MY MAXIS | 失败 | 远端 SIP 返回 `403`，属于网络/订阅拒绝 |
| `+380 772667718 乌克兰`（Kyivstar） | MY MAXIS | 失败 | bearer/REGISTER 观察窗超时 |
| `+372 57006039 爱沙尼亚` | MY MAXIS | 失败 | REGISTER 初始响应超时 |
| `+63 9668305001 菲律宾`（Globe） | MY MAXIS | 失败 | REGISTER 初始响应超时 |
| `+49 15225376999 德国`（Vodafone） | MY CELCOM | 失败 | 已完成 bearer、P-CSCF 和 AKA 流程，最终 `volte_digest_nonce_missing` |

### 结论

Skinny 已从修复前的失败变为成功，证明本次 QMI UIM application AID、逻辑通道 TLV 和 indication 排空修复有效。其余失败均没有引发基带 fatal；从诊断看分别属于未注册、未订阅、远端 SIP 拒绝、网络超时或运营商侧 AKA 响应问题，不能通过修改本地兜底配置直接判定为 SimAdmin 缺陷。

## 德国 Vodafone VoWiFi

原始 Vodafone Profile 的 ePDG 域名在当前漫游网络上无法依赖默认 DNS 解析，因此没有把原始 Profile 当作成功候选。使用用户复制的 Profile（Profile ID：`profile-vodafone-de-26202-7ab2e0d40c`）进行两类替代测试：

| 方式 | ePDG 目标 | 结果 | 失败阶段/日志 |
| --- | --- | --- | --- |
| 自定义 DNS | `epdg.epc.mnc002.mcc262.pub.3gppnetwork.org` | 失败 | 已进入 IKE_AUTH；出现 `ike_auth_notify_no_proposal_chosen` 或 UDP 500/4500 超时 |
| 固定 IP | `139.7.117.168` | 失败 | 已进入 IKE_AUTH；UDP 500/4500 超时 |
| 固定 IP | `139.7.117.169` | 失败 | 已进入 IKE_AUTH；UDP 500/4500 超时 |
| 固定 IP | `139.7.117.170` | 失败 | 已进入 IKE_AUTH；UDP 500/4500 超时 |

四种方式均得到 `vowifi_auto_restore_exhausted`，但不再出现 `sim_auth_logical_channel_failed` 或 `sim_auth_logical_channel_close_failed`。这表明 UIM 读取和逻辑通道生命周期问题已修复，当前阻断点在 ePDG/IKE proposal、UDP 500/4500 可达性或 Vodafone 漫游侧策略。固定 IP 只能绕过 DNS，不能绕过 ePDG 的 IKE 协商和运营商授权。

## 本次修复涉及的 UIM 行为

- Open Logical Channel 使用完整 eSIM Application ID，并匹配 `AID -> Slot` TLV 顺序。
- Close Logical Channel 使用 Channel ID TLV `0x11`，并在响应前排空 `0x0043` 异步 indication。
- 从 UIM Card Status `0x002F` 解析 profile-specific 完整 AID（当前 Vodafone：`A0000000871002FFFFFFFF89`）。
- 主要路径不再静默忽略逻辑通道关闭失败，新增 `sim_auth_logical_channel_close_failed` 诊断映射。

