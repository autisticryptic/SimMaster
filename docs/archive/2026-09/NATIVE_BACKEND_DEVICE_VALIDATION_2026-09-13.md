# 1.1.5 Native 自有设备实测与交接

> 历史档案：本文件保留对应日期的事实，旧版本、worktree 和操作步骤不代表当前状态。
> 当前接手请读 [HANDOFF](../../HANDOFF.md)，不要重放旧部署、回滚或设备命令。

> 本文不含凭据。2026-09-13 用户新授权在**自有设备**临时停用 MM、部署候选并实测；
> 不涉及朋友的 Cloudflare 设备。此前“硬件验收延期”是上一阶段的真实状态，不回填为通过。
> 分支：`dev/1.1.5-modem-backends`。时间均为 Asia/Shanghai。

## 1. 安全边界与部署方式

- SSH 管理走 Wi-Fi，不依赖被测试的蜂窝承载；不重启 NetworkManager/WLAN。
- 原 `simadmin.service`、ModemManager、modem-recovery timer 临时停止；
  MM 仅 `mask --runtime`，没有卸载或永久禁用。
- 原程序/配置/数据库不覆盖。候选使用独立目录、克隆数据库、回环地址 `:13000`。
- 克隆库中的普通数据、VoWiFi、Trunk、eSIM 写操作、自动化、通知均关闭；
  IMS 按测试步骤单独启停。没有主动拨号、发送短信、改 PIN/PUK/FDN 或费用设置。
- 保留 DATA6 初始化服务，不能为“停 MM”连它一起停掉。
- 维护窗口有自动回滚 timer；有未解决 native session receipt 时**保持 MM 停止**，
  不盲删 receipt，不抢占不确定的固件会话。
- Rust 构建/测试/打包仅在 Actions 执行，本地只做 Python 边界检查、rustfmt 和差异检查。

私密连接材料、host key、原始日志和操作脚本保存在本机专项交接目录，不随 Git 分发。
设备重新连接后先核对服务、timer、boot ID、receipt、管理路由，不能直接重放旧命令。

## 2. 固定测试对象

| 项目 | 观测 |
| --- | --- |
| 设备 | MSM8916 / Qualcomm 410，Debian 13，arm64 |
| 内核 / 工具 | `6.17.0-rc6-lkiuyu-compile+` / qmicli 1.36.0 / MM 1.24.0 |
| 卡别名 | `OWN-SIM-A`，不冒认为朋友设备的 SIM-01～03 |
| 归属 / 访问网 | 26202 / 50219，LTE roaming |
| 稳定身份 | 沿用 `physdev:qcom-soc` 与原 line ID，不新建“另一条线路” |
| 主 QMI / IMS | `/dev/wwan0qmi0` / 专用 `wwan0` |
| DATA6 / 普通数据 | **`/dev/wwan0at2` 实际承载 QMI** / `wwan1`；不能按文件名当 AT |
| 真正 AT | `/dev/wwan0at0`、`/dev/wwan0at1`，sysfs type=AT；已确认 termios 可用 |
| UE | mandatory worker / netns 继续沿用，没有宿主网络 bearer fallback |
| 原版 | `1.1.4-beta2` |
| 初始候选 | `b94c9c2`，包内版本仍为共同基线 `1.1.4-beta3`，不是正式 1.1.5 发布 |

初始候选来自 Build-Release run `34740891971` 的 arm64 artifact，
已核对 GitHub API artifact digest 与下载 ZIP 的 SHA256：
`71fa8efb8dd60562fbb816177a163a6435883d60d4383f390eeb484e20e57f1b`。

原程序 SHA256：
`e5e6828d292d988ae8e2db87089cd05bd58762a1b1350c0b1ab4132dde235fe7`。
原数据库使用 SQLite backup API 备份，`integrity_check=ok`，不是在线复制 WAL 文件。

## 3. 实测时间线（保留失败）

### T01 — b94c9c2 基础接管与射频

- 14:39 起，MM inactive / masked-runtime，候选 `active_backend=native`，无自动 fallback。
- 发现一个真实 modem；稳定 line ID 与原配置一致；可读 SIM 身份、NAS 漫游/LTE 信息。
- UE worker/netns ready；只读 backend / modems / line-controls API 正常。
- 14:51:46 开飞行模式：requested=true、observed=true、radio=off。
- 14:51:52 关飞行模式：requested=false、observed=false、radio=on；
  随后先 searching，再回到 LTE roaming；Wi-Fi SSH 保持。
- 尚未做开机持久化、基带重启/掉线长稳、电话、短信或 MBIM/AT-only 验收。

### T02 — 第一次 Native IMS 尝试，失败在承载前段

- 15:03 起只开启候选库该线路及 IMS，普通数据/VoWiFi/Trunk 保持关闭。
- 无 carrier catalog，requested database 回退 derived：
  `derived_3gpp_lte_26202`；归属网与访问网分开解析，没有用漫游网猜 IMS 归属域。
- AT 未配置时，SIM/UIM 身份兜底能够继续；AT PDP profile 不可用时按 APN-only 尝试。
- IPv4v6 / IPv6 / IPv4 均停在 bearer 阶段，
  `native_command_outcome_unconfirmed`；**没有 SIP 注册或 AKA 成功证据**。
- 停止本次重试后检查，没有遗留 native session receipt。

### T03 — 有界诊断定位 QMI 物理通道寿命

停候选，MM 保持停止，只分配/释放诊断 WDS CID，不执行 Start Network：

1. 连续独立 qmicli 进程：CID 分配成功，下一次 Set IP Family / Get Status 超时；
   Release Client 打印 `InvalidClientId`，但 qmicli 的退出码竟仍为 0。
2. 独立发现参数错误：`--wds-set-ip-family=ipv4` 不合法，qmicli 要求 `4` / `6`；
   Start Network 的 `ip-type` 也应使用数字，与 MM/MBIM 拼写不同。
3. 15:16，增加一个持续存活的 **proxy-open socket**（不分配业务 client）后，
   同样的 CID 分配 → family=4 → disconnected status → bind a2-mux-rmnet0 →
   Release Client 全部成功，release 无错误。
4. libqmi 实现对照确认：最后一个 socket 使用者退出时，proxy 会关闭物理设备；
   `--client-no-release-cid` 本身不保证 BAM-DMUX 控制节点一直打开。

因此本次修复目标是**物理 QMI 通道持续持有、命令参数与退出结果判断**，不是给这张卡
硬编码注册参数。T03 是诊断对照，不当作新程序已完成 IMS 注册的证据。

## 4. 本轮代码修复与验证边界

- `6af624c`：
  - Native 普通数据接受既有 `ApnConfig.protocol=dual`，保留 ipv4/ipv6/ipv4v6。
  - QMI/MBIM 优先用自身协议读信号，不强制依赖 AT+CSQ；未知值不伪造为有效信号。
  - Native 存储短信接收增加 `sms_reception_enabled`，**默认 false**。
    仅配置 AT 口不会初始化/扫描/删除存储短信；正式启用接收需显式设置 true。
  - SMS 初始化/扫描遵守线路 enabled/present 与 IMS 接收策略；
    延迟删除前重新核对线路开关，低层仍保留 SIM/存储内容一致性检查。
  - 本地50项 Python 检查通过；对应 Validate run `34744300707` 已通过。
- 后续 QMI 修复：
  - 按物理 controller 持有主/次 QMI 端点的 proxy-open lease。
  - proxy epoch 丢失时 fail closed，不以旧 CID 自动连接新 proxy。
  - 有崩溃 receipt 时不先打开/重置通道；仍需受控 reconciliation。
  - qmicli 参数使用数字地址族；退出码0不能掩盖 Release Client 失败。
  - 正在通过 Actions 与新候选实机复测；下方应追加最终 commit / CI / 部署结论。

## 5. 续接步骤

1. 先核对当前维护窗口是否已回滚，不能假定 MM 仍停止。
2. 检查最新功能 commit 与 Actions，而不是只看版本字符串。
3. 下载 arm64 artifact 并核对 digest；升级独立候选，保留原版本。
4. 新候选先维持 `sms_reception_enabled=false`；若开启 AT，只用于身份/IMS 查询。
5. 依次验证 CID 持有、IMS 承载/IP/P-CSCF、UE 归属、初始注册、原会话自然续期。
   不能拿诊断探针、承载地址、初始重连冒充 SIP 注册或续期。
6. 普通数据在独立 DATA6 端点验证，不把 IMS 挪到 DATA6。
7. 结束维护时优先恢复 MM 默认，核对原配置/数据库/二进制和管理网络；
   如果存在不确定 receipt，先核验，不强行恢复双 owner。

仍不能宣称完全替代 MM：混合 owner、自动代次/孤儿资源恢复、
MBIM/AT-only 数据面、厂商差异、多槽/MEP、业务和长稳矩阵需独立完成。
