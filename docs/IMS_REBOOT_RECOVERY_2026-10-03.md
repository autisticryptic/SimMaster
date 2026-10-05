# 410 重启恢复与 eSIM 显示修复（2026-10-03）

## 最终部署及验收

访问 **http://192.168.68.1:3000**，可强制刷新查看新页面。

- 正式程序为本地已验证源码快照 `b3e96c648a1df6e3ea86a7bebbc21ff2e86ddef1`，版本 1.1.5，PID156230。
  快照保存在 `refs/build-snapshots/worktree-20261003-59f40430ee42`，没有移动工作分支 HEAD、改用户索引或推送 SimAdmin。
  这是本地构建，不是新的 Actions/Release。源码摘要 `59f40430ee423c5e0dabe1166e34b1f6229808614f5512236611c49e29f75969`。
- 包 SHA256 `8c70be34e7692bd2cf73a9e596b32e5ea5f5c96c57e680bb776109b600aba10b`；
  程序 SHA256 `b6c84831b14b03c8384188c567109b22eb8bc15c80b1642c9b96e8fc970e746e`。
- 20 项前端资源的磁盘与 HTTP 摘要一致；保留旧内容哈希 assets 兼容旧标签页。
- 最终只读采样：`registered=true / ipsec / last_error=null / reconnect_count=2 / register_refresh_count=1`。
  当前会话从设备时间 `03:10:19 UTC` 注册，设备时间 `04:00:20 UTC` 实际 REGISTER 自然续期成功，
  protected=true、lease3600秒。最终采样该会话约56分钟。设备时钟与采证机有偏差，不混用两端时间算持续时长。
- **并非从部署开始从未断线**：先前验收会话之后，日志出现 `qca410_primary_mm_data_interface_changed`；
  程序自动重新注册成功。当前会话和续期不能冒充最早同一次会话。
- MM仍PID957，boot未变，整个当前boot内核未查到所筛选的fatal/crash记录。
  没有重启MM/基带、禁IPv4或清冷却预算。仍申请IPv4v6，网络实际授予IPv6。
- 配置表、配置文件与当前Pixel精简库保持；未因后续精简审查更换设备数据库。
- 本次安全守卫最终 `accepted`，recovery timer恢复active，临时secondary抑制drop-in已删除。
  secondary失效重启循环已停止，安装了canonical `device-init` unit；当前MainPID0，不在线执行硬件初始化。
  systemd可保留此前failed状态及历史重启计数，这不是仍在循环。

## 修复内容

### 重启后的旧 IMS 账本

旧账本跨boot仍指向已退出的运行进程，原名namespace又被旧程序创建，阻断正常恢复。
新路径只在真实boot变化、同SIM和同物理控制拓扑、旧owner不存在、完整两次库存一致、
自有profile/reporting与承载/网络资源均证明不存在时，**归档元数据而不删除modem profile**。
仍存在、CID复用、身份不明、创建/删除等不确定阶段继续阻断。

补齐两个恢复缺口：

1. 开机时MM/SIM尚未枚举不能先建UE namespace/worker。所有registry refresh入口共用前置门控；
   未证明安全则不创建，后续每次刷新至多执行一次证明并保留5秒冷却，不做额外REGISTER/基带重试。
2. 崩溃若发生在归档已同步、active名尚未移除之间，只接受权限、归属、单链接和完整字节均匹配的旧归档续接；
   不覆盖证据，错误/截断/symlink/hardlink继续保留active账本。删除active名之前同步归档目录及父目录。

现场部署前仅清理严格双快照证明为空闲的确定namespace/veth；无进程、无XFRM、无硬件网口。
旧账本保持到新正式程序启动，日志已验证程序自动归档成功。

### EID与容量

- 合法32位EID显示首6位、末4位，中间掩码；仅提供完整值复制按钮，不提供显示切换。
- 非法/过短值不通过切片意外暴露完整身份；完整EID不写入DOM属性。
- 删除自定义总容量UI、store操作及配置/API字段；旧缓存中标记为自定义的容量不再返回。
- 剩余存储仍显示芯片实际返回值；详情/重命名/切换/删除的互斥和读回保护保留。

## 测试及真实失败记录

- 312项定向Rust、259项目Python、35安装测试、35前端单测、8隔离浏览器用例通过；
  前端及E2E类型检查、全量lint、前端构建和ARM64构建通过。
- 首次新增端到端测试被本机另一个SimAdmin进程正确阻断，之后在私有PID/DBus环境全部通过；
  没有停掉本机其他程序或弱化生产进程检查。
- 首次部署停服后因secondary是`failed/MainPID0`而不是`inactive`在预检停下；核实无进程与无承载后续接，未重放停服。
- 首个实机窗口虽然已连续注册，但私有收尾脚本错误地索引API省略的`last_error`字段，导致验收未及时结束；
  600秒守卫按deadline停服。**这不是内核fatal或新程序注册失败，也不能隐去该中断。**
  修复观察器后重新核验同boot已知自有账本，第二有界窗口正常恢复注册并连续观察180秒以上，
  同一脚本完成守卫确认、恢复timer、移除drop-in及HTTP校验，后继自然续期也通过。

## 未覆盖与后续

- 本轮没有再次主动整机重启做故障注入；跨boot归档针对现场已有旧账本完成实机验证，启动瞬态和归档中断由回归覆盖。
- 不承诺所有遗留资源自动清理；仍存在的跨owner profile、无法证明空闲的namespace必须继续维护。
- 本轮未验收通话/音频、所有SIM与运营商；后继数据接口变化的根因未单独定位，不称完全消除掉线。
- 数据库 `c445d53` 与报告修复 `f9cc1d3` 已推送独立仓库。minimal审查见
  [精简版差距审查](CATALOG_PRUNING_AUDIT_2026-10-03.md)，报告修复不等于扩大了删除覆盖。

证据根目录：`.local/evidence/continuation-20261003/`。重点为 `source-snapshot.json`、
`package-verified.json`、`deploy-stage.json`、`deploy-install.json`、`deploy-acceptance2.json`、
`closeout-readonly.json`、`later-reconnect-log.json`、`final-verified.json`。失败记录保留。
设备备份：`/opt/simadmin-staging/worktree-20261003-59f40430ee42/backup/`，不要当作孤儿资源清除。
