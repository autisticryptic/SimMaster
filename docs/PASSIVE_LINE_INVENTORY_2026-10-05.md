# 无SIM／IMS恢复受阻时线路展示修复与部署

## 已完成与未完成

最新正式版本 **db54abc / 1.1.5** 已部署到410 **http://192.168.68.1:3000**。
本次解决“物理modem仍在，但网页整条线路消失”；**当前IMS尚未注册，不能把部署成功当成注册成功**。

## 根因

设备在前次部署后重启，启动时保留旧boot的IMS profile账本。同时正常的
`simadmin device-init` DATA6辅助监视进程在运行，严格恢复检查返回
`mm_ims_profile_lease_other_simadmin_running`，启动门禁为 `ims_startup_recovery_pending`。

原registry在发现modem之前等待恢复门禁，因此线路列表API返回503；UI没有可展示的线路。
这不是正常拔卡语义：物理设备存在应仍能显示。普通MM发现代码本来允许无SIM，但未到达该步骤。

用户明确说刚拔出卡。12:53 UTC最新查询仍有MM缓存的46002/SIM对象，但 `AT+CPIN?` 返回
`org.freedesktop.ModemManager1.Error.MobileEquipment.SimFailure: SIM failure`，MM注册状态为搜索网络（u2）。
缓存的IMSI/ICCID/OperatorCode不能证明卡已插回或可用，不能据此触发新SIP认证。

## 修复设计

- 新增 `discover_passive`：默认不支持，不允许偷偷调用可能读取UIM/APDU的常规discover。
- MM实现只调用一次有5秒超时的GetManagedObjects，读取已有属性并用纯函数生成物理槽ID。
  不查询SIM文件、启动进程、读取logical channel、创建transport、写配置或启动UE runtime。
- `passive_inventory_if_blocked` 与正常运行线路map分离，不把仅供展示的设备插入操作registry。
- 线路API阻塞时返回200、`data: []`、`display_only_lines`和`blocked_reason`；旧客户端不会误把展示卡当成可操作线路。
- 前端单独渲染被动设备卡和恢复提示，不挂载SIM/IMS/网络/配置操作控件，不伪造“已禁用”的配置开关。
- `sim_missing`只有MM明确根路径 `/`才表示缺卡；缓存报告有SIM时明确显示“MM缓存报告SIM存在”，
  并提示缓存可能未更新，不把它包装成实卡检测。
- 门禁状态读取不等待正在进行的恢复长操作；互斥锁繁忙时保守视为只读，不启动另一轮恢复。
- 真正worker/namespace/bearer/写卡准入门禁保持，详情/写接口不能根据展示line_id获得操作权限。

## Actions验证

直接提交master，没有新增临时分支：`db54abcb418569d9eb6ad8ce18bb8cc6b715701d`。

- [Build37309875238](https://github.com/autisticryptic/SimMaster/actions/runs/37309875238) success。
- [Validate37309875266](https://github.com/autisticryptic/SimMaster/actions/runs/37309875266) success。
- [Frontend37309875262](https://github.com/autisticryptic/SimMaster/actions/runs/37309875262) success。

两套日志76项定向回归（68既有+7被动库存+1门禁并发）、24+18+12注册矩阵通过；
双架构包官方artifact SHA、30文件清单、ELF/meta和源码指纹全部核验。
ARM64 artifact11345269219，包SHA256 `4c8286f61ed0b729daa4ef34dc5e1e7d1025951a62f1aa9e78372d6971679385`。
全部编译和测试二进制运行均在Actions；本机只运行Python静态检查，没有本机编译。

本地完整静态套件曾因用户私有eSIM文档移动导致原archive链接缺失而有1项失败；没有为了过测试恢复或提交私有文档。
CI中的原受控archive文件存在，该项及所有新测试均通过。

## 实机部署与收尾（12:53 UTC）

- 主PID **34800**、NRestarts0，程序SHA256
  `4cae188b2d318e7cf9003e835081ed0946a00bd38360dfd44eb6732efbc3a5a0`。
- 同PID持续观察超过180秒及独立closeout通过；API为200，`display_only_lines`包含1条物理线路，
  `blocked_reason=ims_startup_recovery_pending`，操作data为空。不是把旧门禁静默取消。
- 20前端资源磁盘和HTTP SHA一致；配置表、运行catalog、停服复制窗口config.yaml/data.db保持。
- 只停止/启动主服务，并临时暂停恢复timer；timer已恢复active，设备侧守卫accepted。
- MM仍PID550/:1.18、boot未变；辅助监视进程仍PID347，未重启MM/基带或辅助硬件初始化。
  因程序文件被原子替换，辅助进程仍持有旧程序映像，`/proc/.../exe`显示deleted属正常Unix行为，不是未知进程。
- 原跨boot账本SHA `32bab7cd066382750547498648bc14fa0b74a10e555dcb048d92a04f7b1daf53`保持，
  未清profile/账本/预算；当前无UE namespace/worker或新bearer。
- 备份在 `/opt/simadmin-staging/passive-inventory-20261005-db54abcb4185/backup/`。
- 本次没有实网SIP注册尝试，未验证CMCC 421候选是否解决用户的那次拒绝。

证据：`.local/evidence/passive-inventory-20261005/{verified,ci-proof}.json`及
`deployment/{package-verified,deploy-stage,deploy-install-accept,deploy-closeout}.json`。
设备状态证据：`.local/evidence/master-deploy-20261005/{removed-sim-readonly,sim-presence-readonly}.json`。

## 注册后续条件

1. 用户将测试SIM插回并确认可被设备正常读取；若仍SIM failure，先检查卡/卡座/接触，不拿缓存身份重试IMS。
2. 卡就绪后重新核验当前MM、SIM、库存、profile活动和原账本，才能选择安全absence归档或显式计划恢复。
3. `device-init`不是任意可忽略的进程：初始化阶段会修改硬件，之后才进入只读监视。
   不能仅凭argv或程序名放宽恢复检查。当前版本可在明确受控维护窗口暂停该服务、证明退出后恢复账本，
   再按记录恢复辅助服务；不能在SIM未知时先删账本。
4. 正常驻网后，才能用已包含的CMCC421修复发起有界注册并核验结果。若失败，保留新安全诊断，不无限重试。

**截至本记录，注册修复仍待SIM就绪，未完成实网验收。**
