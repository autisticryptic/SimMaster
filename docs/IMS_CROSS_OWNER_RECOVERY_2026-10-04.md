# MM/SIM 变化后的 IMS 资源恢复补强

## 正式部署结果（2026-10-04 16:40 UTC）

已按用户明确授权部署到410 **http://192.168.68.1:3000**，正式版本1.1.5 / **71c970d**，
PID769258，运行程序SHA256 `acf44ca39f074ea63c0d9529f933bb1b8e048a5206011913be576ca9e338fe12`。
使用下文已验证的Actions ARM64包，没有本机编译或临时修改二进制。

- 部署前旧Globe会话健康、无通话/启用任务；备份程序/前端/meta、SQLite一致性副本与原账本。
- 暂停recovery timer、正常停止SimAdmin后，旧profile/bearer已清理、无recovery事务残留。
  **未为本次升级执行手工资源删除或跨owner恢复命令**。
- 只更新程序、对应前端和meta；config.yaml/data.db/运行catalog在停服复制窗口哈希保持。
  正式启动后四配置表指纹保持，没有更换运行数据库。
- 新会话16:31:57 UTC实际derived/IPsec注册、实际IPv4；连续稳定观察后，16:40:52仍同一会话，
  last_error=null、reconnect_count1、NRestarts0。requested/owned family4，默认双栈→IPv6→IPv4保持。
- 20项前端磁盘与HTTP资源SHA256核验通过；新活跃账本绑定当前PID/boot，CID3是正常自有资源。
- MM始终PID474743、owner:1.511、原boot保持；未重启MM/基带，部署起点至收尾无新kernel fatal。
- 设备侧600秒守卫已accepted退出，原recovery timer恢复active；没有新增持久maintenance hold/drop-in。

备份：`/opt/simadmin-staging/reconciliation-20261004-71c970df703f/backup/`。
证据：`.local/evidence/ims-reconciliation-20261004/deployment/{package-verified,deploy-stage,deploy-install-accept,final-verified}.json`。

此结论证明新正式版本正常注册与部署保持项，**不是跨owner故障注入实测**。
新会话自然续期仍0，不能借用旧版本2次续期；本轮通话/音频未验收。

## 范围与归属边界

本次增加独立的跨MM owner恢复事务，不放宽原 `identity_io`、`same_binding` 或 `release_with`。
**AT创建的profile没有写入唯一归属标签**：相同CID/完整配置/指纹也不能排除“被其他程序删除后又同样重建”。
所以本次不会凭相同指纹自动删除仍存在的跨owner profile。

| 状态 | 处理 |
|---|---|
| 原owner/SIM仍有效 | 保留原有归属严格的清理路径 |
| owner已退出、旧profile和reporting确实不存在、其他证明齐全 | 自动归档旧账本；支持同boot或换SIM |
| 旧profile仍存在且精确匹配、明确inactive、其他库存/EPS未变 | 提供检查plan；只有显式批准匹配plan才可处理 |
| 已批准事务中断 | 只读核验已发命令结果；同一事务不重复发送reporting/Delete |
| 活动/未知CID、目标活动状态缺失、其他库存变化、owner/SIM再次变化、损坏记录 | 保留阻断及证据，明确提示核验恢复 |

这不是“任何异常都能自动恢复”。没有排他写入历史，无法可靠推断仍存在profile的所有权；
明确授权的精确维护是必要边界，不以修改旧账本的owner/SIM冒充证明。

## 无打扰准入条件

新事务只在可证明的空闲窗口运行：

- 持有原设备flock，只有一个MM modem；无其他manager/worker、live Context、pending bearer或相关账本。
- 原owner已消失，当前owner/SIM/slot/物理拓扑稳定，原创建进程已退出；同进程仅允许明确abandoned且无live Context。
- 无MM bearer或通话、无旧网络地址/路由/规则残留。
- 不存在named namespace；所有进程均在主网络namespace，排除脱离名称但仍被进程持有的namespace。
- 主namespace无XFRM state/policy。其他容器/租户或无法检查的状态保守拒绝。
- 完整AT/MM profile、非目标库存、EPS、reporting与原快照逐项核验，目标CID由原账本确定而非固定CID3。
- present目标必须有明确 `+CGACT: <cid>,0`；缺行不等于inactive。修改分发前再次检查。

**代码不会自动停止服务、清namespace、重启MM/基带或停止其他线路。**
运行期间若仍有worker/namespace，保留阻断；需先安排受控空闲维护，不能在connect调用中边拆网络边恢复。
启动前置门禁现在也覆盖同boot的未结案v2账本/事务，避免先创建worker/namespace把恢复条件堵死。
原门禁5秒冷却仅约束只读检查；没有无限重放写命令。

## 一次性事务与中断

原始 `.json` 账本保持不变；每个设备只有一个固定 `.recovery` 活动事务标记。
以原始字节摘要绑定源记录，不能通过修改源文件另开事务来重置命令次数。

状态为：`Prepared → ReportingDispatched → ReportingConfirmed → DeleteDispatched → AbsentVerified`。
每次命令前先原子写入并fsync文件/目录：

- reporting最多发送一次。超时后只在精确读回000时推进；否则保留人工核验状态。
- Delete最多发送一次。超时后只在双快照absence时推进；仍present绝不重发。
- 当前owner/SIM/boot再次变化、来源/其他库存变化均不能拿旧plan续作。
- 双快照absence完成后才归档原字节；随后把事务记录保存在 `retired/reconciled-*.journal`。
- 若在归档/清活动标记之间崩溃，只有原始归档完整、摘要相同且事务为终态才可完成元数据收尾。
- 孤立/损坏 `.recovery` 也阻断普通分配、切卡以及其他维护命令，不能绕过一次性预算。

## 维护入口（仅受控窗口）

新增后端命令：

```text
simadmin mm-ims-profile-lease --action inspect-stale \
  --modem <当前完整Modem对象路径> --device /dev/wwan0qmi0 \
  --family <原账本请求的族> --apn <原账本APN>

simadmin mm-ims-profile-lease --action reconcile-stale \
  --modem <同一对象路径> --device /dev/wwan0qmi0 \
  --family <相同族> --apn <相同APN> --expected-plan <检查返回的plan>
```

命令不替用户停服，其他manager/worker运行时直接拒绝。不要把示例复制成固定modem/CID或族。
`inspect-stale`不修改modem；首次present恢复缺少/错误plan会在落盘事务和发命令之前拒绝。
已批准事务可再次调用 `reconcile-stale`进行只读结果核验，不会因为再次确认plan而重置命令次数。
这是CLI维护能力，**没有增加网页上的无条件“强制清理”按钮**。

前端为新恢复错误提供“旧 IMS 资源待核验恢复”提示；不误报成运营商注册拒绝，基带故障的优先级保持。

## 验证与部署

编译、Rust测试、前端测试和注册模拟仅在GitHub Actions执行；不在本机编译。
新增测试覆盖授权plan、ABA同值profile不得自动认领、active/遗漏CGACT、库存漂移、持久化失败、
取消/丢回复、防重放、孤立事务、终态归档和原来源字节不变；保留原换卡屏障与24+18注册矩阵。

最终验证快照 **`71c970df703f0a76f1b967bf69d0cea96537a713`**，分支
`dev/ims-reconcile-20261004T154159Z-c60d5e12`：

- [Build-Release 37214027592](https://github.com/autisticryptic/SimMaster/actions/runs/37214027592)：success。
- [Validate Beta Refactor 37214027581](https://github.com/autisticryptic/SimMaster/actions/runs/37214027581)：success。
- [Frontend Checks 37214027570](https://github.com/autisticryptic/SimMaster/actions/runs/37214027570)：success。
- 两套日志逐名核验 **24项新增恢复测试＋19项此前相关测试**，以及24标准/18历史注册场景。
- 新增测试还验证：已归档旧账本被从备份恢复后，不能获得新的写命令预算；不可信归档目录不能隐藏历史。
- ARM64 artifact11307658486，包SHA256 `488bdcdb036e621fd093025c2a0a7c63c0b029723787b3b83479d5473be8c0ed`。
- AMD64 artifact11307862636，包SHA256 `e85870c058999b0bfa20c54d41b6699076a226e3a5771a280e6e236b71dc665e`。
- 官方artifact摘要、包内30文件SHA清单、版本/commit、ELF架构和报告源码摘要逐项核验；Publish skipped。
- 独立只读源代码安全审查没有发现具体阻断；不把代码审查等同于设备故障注入验收。

证据：`.local/evidence/ims-reconciliation-20261004/{current,verified,final-proof}.json`及对应ZIP/日志。
前一候选202e38d也曾通过，但最终结果以上述71c970d为准，不能混用。

**以下为正式部署前的观察**：15:48 UTC仍运行3169c7b/PID732029，Globe自14:00:45 UTC
起保持同一次derived/IPsec注册、last_error=null、reconnect_count1、自然续期2次；MM仍474743。
当时未重启服务、切SIM、删活跃资源或进行故障注入；用户随后明确要求部署，最终结果见首节。
两仓用户HEAD/索引保持，未把旧版本的健康记录冒充新版本验收。
