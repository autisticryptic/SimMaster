# 当前交接

本页是唯一当前交接入口。只保留有效状态与下一步；历史验证见[历史摘要](archive/README.md)。
文档不是设备实时状态，旧PID、SIM、网络和脚本均不得直接重放。

## 用户当前要求

1. **修复全局IMS兜底回归**，追查较早版本能注册、后续失败的实际代码变化。
2. **禁止运营商/MCC/MNC专属代码和特例化测试**，不能以某卡模型代替全局兜底验证。
3. 合并/删除重复文档，不继续新增日期流水账和多份“当前交接”。
4. 日常只维护master；所有编译和测试二进制只在GitHub Actions运行，不在本机编译。
5. 原SIM工作台的列表、详情和标签布局必须保留，不能用另一个只读清单替换。

## 源码状态

- 远端：`autisticryptic/SimMaster`，本地remote通常为`simmaster`；`origin`可能是历史本地路径。
- 主线包含全局安全协商、资源恢复、原线路界面恢复及安装器等既有工作。
- `7bdc59f`曾加入CMCC专用首包分支和专用测试，**已按用户要求由`0502395`完整撤销，未部署**。
- **全局兜底回归尚未修复完成**。不得把撤销前的候选CI或其他卡成功当成本问题已解决。
- 私有eSIM报告、诊断日志、JSONL和`.local`不提交，不清理未知用户数据。

## 已查实的回归线索

用户提供成功版本时间线索：9月29日下午14:53之后、曾通过Cloudflare Tunnel连接410。
原Cloudflare长交接文档已合并，原路径及检索方法见[历史摘要](archive/README.md)。

- 该时间附近的代码族为`7c6cf86 / 09edc03`；`fd34edf`为14:49的文档提交。
- `a6a327e`、`7c6cf86`、`09edc03`、`33d16f3`派生配置文件blob相同：
  `cc05393f657ca36747682fe3c6291b33e374129a`。
- 文档中明确Cloudflare成功的是46011（电信），不能冒认CMCC；另有更早SIM-02/46000成功，但日期9月9日。
  用户所述9月29日CMCC成功是重要外部基线报告，精确运行commit/握手尚未从本地文档确认。
- `39b387b`（10月1日16:04 UTC+8）将全局派生LTE首包Require/Proxy-Require从false改true。
- `bdffdef`随后区分主动声明与server-required，改变部分动态候选的可达性。
- `98d0e09`另改完整多机制报价与strict选择，收紧未报价算法接受范围；不是已证实CMCC使用MD5。
- 后续日志明确实际derived、承载/P-CSCF已建立，标准请求421后进入丢失空AKA/声明的generic，再403。
  421的具体Security-Server/Warning未完整记录，不能直接等同于订阅或SIM鉴权失败。

**下一步只围绕全局协议路径：**比较旧/新请求状态继承、初始/认证/续期边界及候选转换；
明确何时保留身份/安全要求、何时停止。不要再按MCC/MNC分支，不猜改realm/算法，不增加无限重试。
修复后用运营商无关的正反例和旧新对照验证，真实同卡验收另行记录。

检索和源码对照：`.local/evidence/cmcc-regression-20261005/{history-review,code-audit,baseline-flags}.json/md`
（实际文件名分别为`history-review.md`、`code-audit.md`、`baseline-flags.json`）。

## 最近已核验的设备记录，不是当前实时承诺

2026-10-05 18:36 UTC，用户自己的410运行 **4bc3f77 / 1.1.5 / PID9311**：

- 原线路列表/详情/7标签已恢复，API正常线路1，`read_only=false`；20前端HTTP资源摘要通过。
- 当时插回的是**Globe51502**，18:26:56开始同一次derived/IPsec/IPv4注册，last_error=null、reconnect_count1。
- MM538及boot未变；辅助monitor在维护窗口停/恢复后PID1409；恢复timer已恢复，设备侧守卫accepted。
- 旧账本先经双absence证明只归档，未删modem profile；升级时同owner清理延后由原有启动恢复完成。
- 配置/运行catalog保持。新会话自然续期0；未验证当时CMCC卡、通话或音频。
- 管理当时走已pin的WLAN，USB路由不在；重新连接必须核实同一机器，不能猜IP或关闭host-key校验。

证据：`.local/evidence/original-line-ui-20261005/` 的`verified.json`、浏览器截图、
`absence-recovery.json`和`deployment/final-verified.json`。
此记录不授权重放旧维护命令，不要把正常新CID3当成旧残留删除。

## 发布与数据库

- 分支已收敛为master；临时验证提交历史保留，旧Release标签未因分支整理被移动。
- 普通push只产artifact；正式Release需要明确授权和完整门禁。
- 独立catalog项目的v0.3.1-catalog-v7已发布并验证，四来源12库、总精简55.70%。
  大部分收益为审计证据瘦身，不是删去同等比例运营商配置；详见[运营商配置](CARRIER_PROFILES.md)。
- 不因本次兜底调查重建库、扩大裁剪或替换设备库；一次程序/配置/库改变要分别归因。

## 文档及验证入口

- [开发与Actions](DEVELOPER.md)：单主分支、禁止本机编译、源码/产物绑定。
- [IMS协议](IMS_REGISTRATION_POLICY.md)：全局注册/兜底/续期与费用策略。
- [MM生命周期](IMS_MM_EXACT_FAMILY_LEASE_DESIGN.md)：归属、换卡、清理与显式恢复。
- [诊断](IMS_DIAGNOSTICS.md)：只读采证和脱敏。
- [未完成计划](DEVELOPMENT_PLAN.md)：长期事项，不以旧勾选记录当成已验收。

本次精简前所有正文已保存在Git历史和 `.local/evidence/docs-consolidation-20261006/docs-before.zip`，
摘要清单为`backup.json`；历史文档不是删除业务证据。私有报告原件仍由用户在本地保存。
