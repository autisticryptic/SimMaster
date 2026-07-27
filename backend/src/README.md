# SimAdmin Backend 源码结构

本目录是 SimAdmin 后端（Rust / axum）的全部源码。代码按**功能领域**分目录组织：
每个目录是一个自洽的子系统，`main.rs` 组装它们并启动 HTTP 服务，`state.rs` 持有
被所有子系统共享的全局状态。

## 顶层布局

```
backend/src/
├── main.rs          程序入口：加载配置、初始化各子系统、装配路由、启动服务
├── state.rs         全局 AppState（被所有模块依赖，故留在根目录）
│
├── ims/             共享 IMS 核心：SIP 报文 + Digest-AKA（VoWiFi / VoLTE 共用）
├── access/          接入腿：以不同物理通道接入运营商 IMS 网络
├── automation/      自动化任务：定时/触发式执行（重启、发短信等）
│
├── api/             Web 层：HTTP handlers + 请求响应模型 + 认证
├── cellular/        蜂窝域：ModemManager、小区锁定、串口
├── messaging/       短信域：短信监听转发、验证码提取
├── notify/          通知域：多渠道推送及其发送队列
├── network/         网络域：DDNS、iptables 防火墙
├── system/          系统域：OTA、系统事件、设备状态
├── sim/             SIM 域：eSIM 配置管理
└── infra/           基础设施：配置、数据库、通用工具
```

## 架构理念：共享核心 + 可插拔接入腿

项目的核心目标是用同一个 SIM 号码，通过**不同的物理路径**接入运营商 IMS
网络来收发短信 / 通话。这些路径的上层协议（SIP 信令、Digest-AKA 鉴权）完全
相同，差别只在「如何建立受保护的信令通道」。因此代码分为两层：

- **`ims/`（共享核心）**：所有接入腿都相同的部分，只实现和测试一次。
  - `sip_frame.rs` — SIP 报文组帧 / 解析（状态行、头、粘包切帧，RFC 3261）
  - `digest_aka.rs` — Digest-AKA 鉴权计算（RFC 2617 / 2104 / 3310 / 4169）
  - `mod.rs` — 中立错误类型 `ImsError`（各接入腿映射到自己的错误类型）

- **`access/`（接入腿）**：每条腿特有的「建立受保护通道」的方式。
  - `vowifi/` — WiFi → ePDG → IMS，用户态 IKEv2 / ESP（`ike_*`、`epdg`、`dataplane`、
    `tun_gateway` 等），复用 `qmi_uim` 做 SIM AKA、`sms`/`voice` 做业务。
  - `volte/` — LTE 基带 → IMS APN bearer → IMS，内核 `ip xfrm`（`ipsec`、`bearer`、
    `pcscf`、`sip`、`digest_aka`、`sms`、`voice`、`rtp_relay` 等）。

未来新增 **ViLTE（视频）** 或 **CS（基带直连）** 时，只需在 `access/` 下新增
目录并复用 `ims/`，无需改动共享核心。

## 各领域目录说明

| 目录 | 职责 | 主要文件 |
|------|------|----------|
| `ims/` | 共享 IMS 信令与鉴权 | `sip_frame`、`digest_aka` |
| `access/vowifi/` | VoWiFi 接入腿（IKEv2/ESP over ePDG） | `ike_*`、`epdg`、`dataplane`、`live`、`profiles` |
| `access/volte/` | VoLTE 接入腿（内核 IPsec over LTE） | `sip`、`bearer`、`pcscf`、`ipsec`、`sms`、`voice` |
| `automation/` | 任务调度与具体任务 | `scheduler`、`traits`、`tasks/*` |
| `api/` | 对外 HTTP 接口 | `handlers`、`models`、`auth` |
| `cellular/` | 调制解调器与蜂窝控制 | `modem_manager`、`cell_lock_store`、`serial` |
| `messaging/` | 短信收取与处理 | `sms_listener`、`verification_code` |
| `notify/` | 通知推送与队列 | `notification`、`notification_queue` |
| `network/` | DDNS 与防火墙 | `device_network`、`iptables` |
| `system/` | OTA、事件、状态 | `ota`、`system_event`、`system_event_monitor`、`device_status` |
| `sim/` | eSIM 管理 | `esim` |
| `infra/` | 配置、数据库、工具 | `config`、`db`、`utils` |

## 模块引用约定

- 跨领域引用使用绝对路径，例如 `crate::infra::config::ConfigManager`、
  `crate::cellular::modem_manager::...`、`crate::ims::sip_frame::...`。
- 领域内部子模块之间可用 `super::` 相对引用。
- `state.rs` 与 `main.rs` 位于 crate 根，可直接以 `crate::state`、模块名引用。

## 构建与测试

```powershell
# 在 backend/ 目录下
cargo build          # 编译
cargo test           # 运行全部单元测试
cargo clippy         # 静态检查
```

真实网络 / 硬件相关的逻辑（`access/*/live.rs`、`bearer`、RTP 中继等）依赖目标
设备（SIM / 基带 / P-CSCF），仅能在真机验证；报文构造、编解码、鉴权计算、
命令拼装、选路 / 去重逻辑等纯逻辑部分可在本地通过单元测试验证。
