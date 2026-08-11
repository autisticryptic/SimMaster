# 运行环境与系统管理

## 目标设备运行要求

- **操作系统**：Linux / Debian 系统。
- **系统管理器**：systemd。
- **权限**：需要 root 运行权限。
- **IPC 机制**：system D-Bus。
- **核心依赖包与指令**：
  - `ModemManager` 和 `mmcli`
  - `NetworkManager` 和 `nmcli`
  - `qmicli`（用于基站定位/网络小区信息兜底读取）
  - `iptables` / `ip6tables`（仅用于网络通路只读诊断；本程序不会自动修改或清空防火墙规则）
  - `ip` / `ifconfig` / `route`（用于配置 VoWiFi 虚拟 TUN 网关与路由，其中 `ifconfig` 和 `route` 需确保系统已安装 `net-tools`）
  - `/dev/net/tun` 设备支持（VoWiFi 用户态 IPsec 报文传输必需）
- **eSIM 芯片管理**：每线路 eUICC/Profile 管理依赖开源的 `lpac` 辅助程序。
- **VoLTE 独立 bearer**：多线路 VoLTE 需要设备为每个基带暴露可用的副 QMI 控制端点；
  随仓库提供的 `simadmin-secondary-qmi.service` 与 udev 规则面向支持 DATA6 的目标内核。
- **运营商配置库**：服务启动时必须能读取由 `carrier_Bundles` 生成、已封存且契约兼容的
  schema v7 SQLite catalog。默认文件名为后端二进制同目录的 `carrier-bundles.sqlite3`，
  也可通过 `--carrier-catalog` 或 `SIMADMIN_CARRIER_CATALOG` 指定。

---

## 默认安装路径与文件说明

| 路径 | 说明 |
|------|------|
| `/opt/simadmin/simadmin` | 后端二进制程序 |
| `/opt/simadmin/www/` | 前端 Web 静态 SPA 资源文件 |
| `/opt/simadmin/lpac/` | 可选的手动安装 `lpac` 目录，后端优先调用此路径 |
| `/opt/simadmin/carrier-bundles.sqlite3` | 只读运营商接入/IMS/SIP catalog；运行时只接受受支持且已封存的 release |
| `/opt/simadmin/data.db` | SQLite 数据库文件（保存短信记录、登录认证密码散列值、会话 Token、自动化日志等） |
| `/data/config.sqlite3` | 优先使用的用户配置 SQLite；保存 `AppConfig` 与按 SIM 的 IMS 覆写 |
| `/opt/simadmin/config.sqlite3` | `/data` 目录不存在时的用户配置 SQLite 回退路径 |
| `/opt/simadmin/meta.json` | 旧 OTA 流程的元数据文件；手动部署不要求，当前暂停使用 |
| `/tmp/ota_staging` | 旧 OTA 流程的临时目录；当前手动部署不使用 |
| `/run/simadmin/secondary-qmi-endpoints.json` | 各基带副 QMI 端点的临时运行态映射，重启后重建 |
| `/etc/systemd/system/simadmin.service` | SimAdmin 后端主服务守护单元 |
| `/etc/systemd/system/simadmin-secondary-qmi.service` | 可选的开机副 QMI/DATA6 准备服务 |
| `/etc/systemd/system/simadmin-modem-recovery.service` | 开机 modem 搜网异常自愈恢复服务单元 |
| `/usr/local/bin/simadmin-modem-recovery.sh` | 开机自愈监控与搜网状态恢复的执行脚本 |
| `/etc/NetworkManager/conf.d/99-simadmin-unmanaged-modem.conf` | NetworkManager 忽略托管 `wwan*` 接口配置，避免与主服务抢占调制解调器控制权 |

---

## eSIM 芯片管理机制

本项目中的 eSIM 指插入基带卡槽或独立读卡器的实体 eUICC。eSIM 管理已经从旧的全局
“普通 SIM/eSIM 工作模式”下沉到每条线路：

* **自动**（配置值为 `null`）：根据该线路的 `SimType` / `EsimStatus` 探测结果决定是否开放
  eSIM 管理；这是默认行为。
* **启用**（`true`）：用户显式允许该线路使用 `lpac`，用于自动探测不可靠但实际存在
  eUICC 的设备。
* **禁用**（`false`）：拒绝该线路的 eSIM 管理操作，普通 SIM 功能不受影响。
* **按需调用**：只有打开某条线路的 eSIM 管理弹窗，或执行下载、启用、重命名、删除等
  Profile 操作时，才瞬时调用 `lpac`；不同线路通过各自 QMI 设备与 UIM slot 寻址。
* **独立读卡器**：QMI 读卡器可配置为没有蜂窝基带、仅供 VoWiFi/eSIM 使用的独立线路；
  PC/SC 形式是否可用取决于部署的 `lpac` APDU 后端。
* **`lpac` 安装与维护**：
  - 当前不提供自动下载或安装。请从可信来源取得与设备架构/libc 兼容的构建，审核后手动
    安装到 `/opt/simadmin/lpac/lpac`，所需动态库放入 `/opt/simadmin/lpac/lib/`。
  - Web 中已有的“安装/修复 lpac”入口依赖旧下载流程，在仓库与制品来源重构完成前不要使用。
  - 手动升级 SimAdmin 不会改动 `lpac`；需要升级时应单独备份、替换并验证。

---

## VoWiFi / VoLTE 运行管理

本项目实现了 WiFi Calling 核心协议能力，无需额外安装其他后台程序即可使用。

* **VoWiFi**：每条线路建立自己的 ePDG、IKEv2/ESP、TUN、SIP 和业务运行时，并通过该线路
  的 SIM 做 EAP-AKA。TUN 名称按 `line_id` 派生（通常形如 `sa_vwf_*`），可为不同线路分别
  配置直连或 SOCKS5 出口。停止一条线路时只释放该线路的隧道、路由和会话。
* **VoLTE**：每条基带线路使用自己的 IMS APN bearer、副 QMI 端点、P-CSCF 和 `ip xfrm`
  状态。普通移动数据与 IMS bearer 分离，具体能否同时工作取决于基带、驱动和运营商。
* **业务选路**：短信可在 VoWiFi、VoLTE、CS 之间排序和回退；IMS 语音在 VoWiFi、VoLTE
  之间选路。策略按线路保存，接收侧会选举 IMS 监听腿并对跨通道重复消息去重。
* **关闭连接**：停止某条线路的 VoWiFi/VoLTE 只清理该线路运行时，不应影响其他线路。

---

## systemd 服务配置说明

### 主服务守护单元 (`simadmin.service`)

默认配置位于 `scripts/simadmin.service`：

- `WorkingDirectory=/opt/simadmin`
- `ExecStart=/opt/simadmin/simadmin`
- `Restart=always`
- `UMask=0077`（约束配置库 WAL/SHM、E911 state 等运行期敏感文件的默认权限）
- `Environment=DBUS_SYSTEM_BUS_ADDRESS=unix:path=/var/run/dbus/system_bus_socket`

### 常用管理命令

```bash
# 查看主服务状态与调试日志
systemctl status simadmin --no-pager
journalctl -u simadmin -f

# 查看开机 modem 恢复服务的日志
systemctl status simadmin-modem-recovery --no-pager
journalctl -u simadmin-modem-recovery -f

# 支持副 QMI 端点的部署可检查此服务
systemctl status simadmin-secondary-qmi --no-pager
journalctl -u simadmin-secondary-qmi -f
```

---

## 数据持久化与存储设计

### 1. SQLite 数据库数据

保存在 `/opt/simadmin/data.db`，主要存储：

- 短信、跨通道去重指纹、SMSC/本机号码缓存和每线路数据流量。
- 通话记录、通知日志与失败重试队列、自动化运行日志。
- 管理员密码哈希、Web 会话、eSIM Profile 缓存。
- VoWiFi 运行事件、快照、短信投递和压测数据。

*注：管理员密码和会话 token 不以明文存储。修改密码或清除管理员配置会同步置空所有旧会话令牌。*

### 2. 本地持久化配置数据库

保存在 `/data/config.sqlite3`（或回退路径 `/opt/simadmin/config.sqlite3`），主要存储：

- 每条线路的 APN、数据代理、流量、漫游、飞行模式、VoLTE/VoWiFi、eSIM、Trunk 和
  语音/短信路径策略。
- 独立 SIM 读卡器、设备网络、WLAN 与 DDNS 设置。
- 通知通道、转发规则、模板、清理策略和限流设置。
- 自动化任务、系统安全策略、OTA/恢复相关配置。
- `ims_sim_overrides` 中按 ICCID 或 EID + profile ICCID 绑定的 IMS/ePDG、自定义 IMEI、
  语音信箱号码和 E911 本地地址意图。

配置库包含号码、代理和通知凭据等敏感信息，文件权限为 `0600`。升级前先停止服务并备份
`config.sqlite3*`，避免漏掉 WAL；只读 carrier catalog、运行数据、用户配置和 E911 secret
state 应分别升级和备份，不要互相覆盖。程序不读取或导入旧 `config.json`。

## 自动安装与 OTA 状态

远程安装脚本、在线版本检查、OTA 上传/应用和自动卸载当前均暂停使用。源码与路径说明仅为
后续重构保留，不表示它们已经适配当前仓库。现阶段部署和升级统一按
[手动安装指南](./INSTALL.md)执行。
