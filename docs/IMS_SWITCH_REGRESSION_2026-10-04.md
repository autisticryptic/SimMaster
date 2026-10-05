# 换卡清理屏障与历史 IMS 注册回归（2026-10-04）

## 最终实机结果：Globe恢复（14:09 UTC核验）

用户要求继续后已执行受控维护，410 **http://192.168.68.1:3000** 正式运行 `3169c7b`，
PID732029、NRestarts0，程序SHA256 `b9867b5762112ea58f367e170c39e0dd0ecd640d740e93a83ac11d17af94cf7a`。
14:00:45 UTC实际注册，连续采样并在14:09:11收尾仍是同一个registered_at、
**derived / IPsec / IPv4、last_error=null、reconnect_count1**。不是维护探针注册。

### 有界维护与原证据保留

旧SIM与当前Globe不同，所以没有修改自动恢复规则来接管旧owner。
先备份程序/前端/meta、SQLite一致性副本与原始账本；停主服务和recovery timer后验证无通话、
无其他manager/worker、无MM bearer或bearer receipt。旧namespace只剩已验证的空闲veth/lo，
无进程/XFRM；核对双快照及peer关联后仅清理该旧namespace/veth。

显式维护计划用双快照绑定当前boot/MM owner/SIM、原账本摘要、完整profile/AT定义、EPS、reporting和CGACT。
CID3必须inactive且AT/MM指纹与旧owned项完全一致，其他CID1/2/EPS完全不变；6项纯fixture测试覆盖
活动/缺失/变更目标、变更EPS/其他记录、部分清理及不允许覆盖一次性调度证据。
只执行一次CID3 reporting000恢复及ProfileManager Delete，失败/不明不自动重放。
清理后完整双快照证明CID3不存在、其他项保持，原账本仍在；再调用已验证程序的
`inspect-retired` + 匹配token的 `retire-absent`，仅归档原账本。
归档与维护前备份逐字节相同，原记录没有改写成新SIM/owner。

随后只安装已校验Actions包中的程序/前端/meta；正式启动由600秒设备侧守卫监护。
取得连续201秒注册、当前进程活跃租约及MM/boot稳定证据后接受，恢复原recovery timer。
当前新CID3由正式程序重新创建、requested/owned family4（双栈），实际获得IPv4；
**它是正常资源，不得再按旧账本清理**，默认双栈→IPv6→IPv4未改。

### 最终检查

- 配置文件、安装停服窗口data.db、运行catalog摘要保持；启动后四配置表逐表指纹保持。
- 20项前端资源磁盘及HTTP SHA256一致；包内安装器/unit未用来改系统服务配置。
- MM始终PID474743、原boot未变，没有重启MM/基带、清预算或替换运行数据库。
- 维护起点至收尾无新kernel fatal，QMI在位；守卫accepted、recovery timer active。
- secondary仍为原来的failed/MainPID0，4199是历史计数，本轮未启动或改它。
- 收尾观察器首次错误地索引被API省略的null `last_error`，已改为可选读取并重新只读核验成功；
  没有因观察器错误重启/重试。失败记录保留，不把它当成设备注册故障。
- 自然续期仍0；没有为验收打断新注册做来回切卡，通话/音频和真实故障注入不在此次结论内。

证据：`.local/evidence/ims-switch-actions-20261004/globe-repair/` 下的
`stage.json`、`stop-inspect.json`、`cleanup.json`、`install-observe.json`、`closeout.json`、`final-verified.json`。

## 执行约束与源码

本轮续接两份10月4日JSONL。**没有在本机编译**；Rust、前端构建及注册模拟均在 GitHub Actions。
本机仅编辑、Python静态/纯fixture检查、下载并校验远程产物；410操作为上述已授权受控维护与核验。

- SimAdmin 工作 HEAD 仍为 `5c378f81d2b5c01b3072c9d3681e14b9fdbd9c90`，用户索引未变。
- 独立验证快照：`3169c7b4878966147779943fc6dd6c9075bcb48f`。
- 远端验证分支：`dev/ims-switch-20261004T120258Z-2785dbf6`。
- 快照包含当前待验证生产源码/前端/测试/打包文件，未将本机证据、数据库或用户文档移动混入提交。
- 未合并 master，用户HEAD/索引保持；SimAdmin包未发布Release，但已部署410。

## 已完成的修复

1. 原 `discard_live_for_mm_binding_change` 清理失败只告警，API仍可能继续lpac和全局MM恢复。
   新屏障必须在任何lpac任务/身份清空/MM变更之前取得：持久profile账本不存在、清理Context已释放
   同一设备flock、全局bearer/pending-create/其他账本均无残留。不解析损坏账本来猜归属，更不删除它。
2. flock跨整个lpac及MM恢复的异步操作持有；成功/失败/取消均由RAII释放。清理不放在serial permit内。
   磁盘目录、符号链接和权限不可信均拒绝，不能将损坏/无法读取当作absence。
3. MM恢复作用于全局，增加registry discovery的独占预留：先冻结新线路发现，再检查当前库存。
   全局MM操作完成后、最终registry refresh之前仅释放库存预留，避免自锁；线路ticket和设备flock仍持有。
   多个已知modem线路或slot冲突保守拒绝，不宣称支持任意多modem热切换。
4. 没有MM admission ticket时禁止只凭账本absence切卡，防止withdrawn binding重新变ready的竞态。
5. `hmac-sha1-96` 与 `hmac-sha-1-96` 按同一完整性算法比较；保留原始Security-Verify。
   不扩大MD5、未报价加密或AES-only的允许范围。
6. 修复上轮CI漏选 `runtime::switch_drain_tests`；两套门禁都运行它和标准/历史矩阵。
   `offline-registration-sim/run.py` 在非Actions环境于创建证据/调用编译器前直接拒绝执行。

## GitHub Actions 验证

以下三个运行均为 **success**，且下载产物与官方artifact digest逐一匹配：

- [Validate Beta Refactor / 37200807900](https://github.com/autisticryptic/SimMaster/actions/runs/37200807900)
- [Build-Release / 37200807750](https://github.com/autisticryptic/SimMaster/actions/runs/37200807750)
- [Frontend Checks / 37200807803](https://github.com/autisticryptic/SimMaster/actions/runs/37200807803)

两套后端门禁的实际日志都证明执行了：

| 验证 | 结果 |
|---|---|
| 标准REGISTER矩阵 | 24场景：14模拟成功，10预期拒绝 |
| 历史条件矩阵 | 18场景：12模拟成功，6预期拒绝 |
| 新换卡/cleanup/flock测试 | 15项通过 |
| 新库存互斥/取消测试 | 2项通过 |
| SHA1别名正反例 | 2项通过 |

报告中的源文件SHA256逐一对照 `git show 3169c7b:<path>`，不是用旧HEAD测试替代脏工作树。
矩阵源树摘要：`4955e8d57f5912f24cf8981c507ca07c49ee089fa9e723c283600aa7787ebda3`。
此外现有安全协商、IPsec、REGISTER续期、私有D-Bus/API、前端/安装器等门禁全部通过。
不能将多筛选器重复执行的累计数当成独立测试数量。

### 历史卡边界

参考 [已入库的历史eSIM报告](archive/2026-09/ESIM_IMS_PROFILE_TEST_2026-09-01.md) 和HANDOFF中后继实网记录。用户在本地移动并补充的私有版本不随本次整合提交。
Globe/KPN覆盖两种SHA1拼写、AES/null及403止损；SIM-01/02/03/04覆盖已记录的IPv4/IPv6
UDP/AKA身份路径；SIM-06覆盖SHA1/AES与别名。异常nonce、错误Digest、未报价MD5和显式禁用须停止。

这些是**合成身份和内存registrar的协议回归**，不是重放真实SIM挑战或断言每张卡一定注册成功。
地址族只影响SIP序列化，不模拟真实bearer/MM/UIM/MTU。Skinny等缺少完整协商参数的历史成功，
以及未注册、未订阅、承载或P-CSCF超时的历史卡，没有凭空生成实网通过结论。

### 双架构包

| 架构 | artifact | tar.gz SHA256 |
|---|---:|---|
| ARM64 | 11303455192 | `813118291273111c1cc02e7a2871f9e2247453aa04187d5aac096d952182dbe5` |
| AMD64 | 11303120895 | `2eb2224dbb1ff42b255c4f23dfb0bad6bfcbf16c9f3f9849a617b7f7f00db5c3` |

已校验官方ZIP摘要、包外SHA、包内30文件SHA清单、ELF架构、版本/commit及同包安装器/unit。
`Publish Release` 为 skipped。ARM64包后续已用于上文受控维护，当前设备运行该已校验程序。

## 历史只读现场（维护前）：未恢复，不能删账本冒充修复

2026-10-04 **11:51 UTC**，通过已pin主机的只读SSH/API/AT查询：

- 410 `http://192.168.68.1:3000`，SimAdmin仍PID348115、旧快照98d0e09；MM仍PID474743。
- Globe home51502、漫游50212，当前没有注册、没有活动通话；未发新的REGISTER重试。
- 旧MM owner`:1.18`已不存在；当前`:1.511`，modem从旧`/Modem/4`变为`/Modem/0`。
- v2账本仍 `probed / cleaning / abandoned`、旧owner/PID、owned CID3及bearer镜像。
- **CID3 `IPV4V6/ims`及reporting `[1,1,1]`仍真实存在**；CGACT显示CID3 inactive不能等同于profile不存在。
- namespace仍有UE worker；XFRM为空不能证明整个旧profile资源已经清理。
- 直接阻断是 `runtime_owner_changed`／`runtime_receipt_pending`，不是已收到Globe的新SIP拒绝。

截至此只读快照尚未停服务、删profile、清预算、换库或部署。用户后续要求继续，才执行首节的
受控维护及独立实网验收；新的屏障不会自动删除已经跨owner/SIM的遗留资源。

## 证据

- `.local/evidence/ims-switch-actions-20261004/{current,verified}.json`
- 同目录各run的标准/历史JSON、regressions日志和官方digest核验后的ZIP。
- `.local/evidence/resume-20261004/device-readonly-20261004T115139.json`
- carrier精简产物进展见 [数据库Actions验证](CATALOG_ACTIONS_2026-10-04.md)。
