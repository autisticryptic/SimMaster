# 历史验证摘要与原文定位

本页合并旧交接、日期报告和重复索引。**历史记录不代表当前设备、当前master或所有SIM已通过。**
当前工作只看[HANDOFF](../HANDOFF.md)，协议设计看主题手册；不把旧Prompt当作新的操作授权。

## 1. 如何找回原文

精简前提交为`05023950ba5f4a7c4283db4ef63a0ba7a20d7818`。所有已跟踪旧文档仍可从Git读取：

```bash
git show 0502395:docs/NEXT_AI_HANDOFF_2026-09-28.md
git show 0502395:docs/NEXT_AI_HANDOFF_2026-09-30.md
git show 0502395:docs/archive/2026-09/PROJECT_HANDOFF_2026-09-12.md
```

本机完整备份：`.local/evidence/docs-consolidation-20261006/docs-before.zip`。
`committed/`保存该提交全部文档，`working/`保存整理时文件，`backup.json`包含逐文件及ZIP SHA256。
其中可能有本地私有文档；**备份不得上传、公开或重新加入Git**。
用户自行维护的未跟踪eSIM报告保留原位置，不包含在新的公开摘要中。

## 2. 用户提到的Cloudflare文档

旧文件`docs/NEXT_AI_HANDOFF_2026-09-28.md`包含：

- §0.3–0.4：通过原Cloudflare WebSocket/SSH连接朋友的QCA410，使用固定host key和既有账户。
- §0.0：`a6a327e`部署、注册和用户确认的来电送达；实际profile **46011**，不是CMCC。
- §0.12：9月29日`7c6cf86`拨号结果分类候选与验证。
- §0.13：9月29日06:43–07:14UTC（14:43–15:14 UTC+8）**另一台LAN设备**重装；不等于CMCC实网成功。
- §0.14–0.15：随后UI与LAN当前eSIM问题；设备、卡和版本不能与Cloudflare结果互换。

其他本地只读来源（不提交）：
`.local/archive/legacy-docs/IMS_DERIVED_FALLBACK_HANDOFF.md`、
`.local/archive/root/.codex-device-notes/qca410-ims-2026-09-08.md`。
凭据不在本页，不因找到旧连接脚本而重放登录后的修改命令。

## 3. 关键时间线

除明确注明外，下表时间按原文语境；代码commit、文档commit、部署时间和设备墙钟必须分开。

| 阶段 | 代码/对象 | 有证据的结论与边界 |
|---|---|---|
| 1.1.4-beta1 | `e55780a` | 原通道自然续期与单注册回退；网络未接受多流，不声称双注册实机完成 |
| 1.1.4-beta2 | `0b97b4f`、`48e37fc` | Hickory服务器顺序修正、应用HTTP DNS与自然受保护续期通过；专用carrier DNS未统一重写 |
| 9月9日Cloudflare SIM-02 | `05de680`、home/serving46000 | 标准派生IPv6/UDP注册，10:18:45→11:08:46 UTC+8同会话自然续期；不是9月29日，也不是IPsec/全业务证明 |
| 9月12日SIM-03 | `684e2a7`、home45403 | MM主承载、IPv4/UDP注册与续期；访问46000不等于中国移动SIM |
| 9月中下旬SIM-04/05 | 多个1.1.5候选 | 保留用户已有验收范围；不据此证明native、所有地址族或全部运营商 |
| 9月28日Cloudflare SIM-06 | `dd8ba1f`、home46011 | MM恢复后的派生IPsec注册和自然续期；不是CMCC |
| 9月28日晚/29日凌晨 | `a6a327e` | Cloudflare设备46011注册及一次来电送达；未接听，不代表音频/通话时长通过 |
| 9月29日14:53附近 | `7c6cf86`/`09edc03`，`fd34edf`文档 | 同一派生配置代码族；附近文档有LAN重装，未找到能唯一证明当时CMCC成功的运行哈希 |
| 9月29日晚 | `33d16f3`、`828135b` | UI及MM实际bearer网卡绑定；LAN卡为Globe51502，阶段性未注册 |
| 9月30日 | `a269e9d`、`cf13a66` | 全P-CSCF路由及旧lease收尾补强；“路由是唯一根因”曾被撤回，真实WDS/零响应仍需区分 |
| 9月30日维护 | `6a77d92`等 | 受控临时profile创建/回收闭环；QMI创建失败与AT创建成功分开，不能等同注册成功 |
| 9月30日/10月1日 | `48e269c` | 正式Globe派生IPsec注册及后续6轮自然续期；注销500独立保留，用户要求不因此打断健康注册 |
| 10月1日 | `6c6fcfd`、`448c98a` | KPN/新卡IPv6与默认双栈申请的后继验证；单族规避不等于固件根因修复 |
| 10月1日全局首包变化 | `39b387b`、`bdffdef` | 主动Require/Proxy-Require及server-required状态区分；当前全局兜底回归重点，不能跳过成功基线比较 |
| 10月3日/4日 | `b3e96c6`、`cc918f5`、`98d0e09` | 启动恢复、派生协商、完整安全报价/strict选择、null密钥修正及当时Globe验证；各次会话和计数独立 |
| 10月4日 | `3169c7b` | 切卡前清理屏障/库存锁及历史条件测试；旧资源受控处理后Globe恢复，不是自动强删 |
| 10月4日后继 | `71c970d` | 跨owner恢复事务、显式plan和不重放；部署正常注册不等于跨owner故障注入验收 |
| 10月5日421候选 | `6ddc751` | 受限安全提示重报价与诊断，CI通过；不是历史CMCC成功版，也未证明真实CMCC恢复 |
| 分支收敛 | `ae3926e` | 累积验证快照保留历史合入master，5个临时分支清理，标签不随之重置 |
| 错误UI尝试 | `db54abc` | 清单可见但替换原工作台，被用户明确否定；不得把API200当原UI修复完成 |
| 原UI纠正 | `4bc3f77` | 原列表/详情/7标签、只读投影和浏览器回归；已部署，当前当时Globe正常线路/注册通过 |
| 已撤销的特例尝试 | `7bdc59f`→`0502395` | 号段首包分支及专属测试被用户否定后完整撤销，未部署；不能引用其测试宣称全局兜底修好 |

## 4. 已公开catalog事实

`carrier_Bundles`的v0.3.1-catalog-v7目标`814b057`，构建37201477372、发布37206201366。
20公开文件和12SQLite验证通过，总no-icons46,387,200→minimal20,549,632字节（55.70%），
小米三变体均保留380条静态WFC ready。主要缩减审计证据，不是额外证明所有运营商注册可派生。
详情已合并到[运营商配置](../CARRIER_PROFILES.md)。

## 5. 合并归属

- 旧日期交接、Prompt模板、部署流水：本页及[HANDOFF](../HANDOFF.md)。
- DNS和namespace说明：[架构](../ARCHITECTURE.md)。
- 运行环境：[安装](../INSTALL.md)。
- 注册/共存/三态/命名/续期/VoWiFi审计：[IMS协议](../IMS_REGISTRATION_POLICY.md)。
- profile生命周期、SIM校准、重启/跨owner恢复：[MM生命周期](../IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)。
- catalog变体、裁剪、Pixel/iOS及小米报告：[运营商配置](../CARRIER_PROFILES.md)。
- 原生后端分项、维护、资源/SIM账本、MEP范围：[原生后端](../NATIVE_BACKEND_STATUS.md)。
- 旧版本发布说明：[CHANGELOG](../CHANGELOG.md)。

旧文档原文没有被用作新的设备操作指令，也没有为了精简篡改失败、未测或用户否定的事实。
