# VoLTE 真机实测结论：QMI 端点能力矩阵与崩溃机制

**实测日期：** 2026-07-27
**设备：** 192.168.100.13（密码 1313144），Debian 13 trixie
**内核：** `6.17.0-rc6-lkiuyu-compile+`
**基带：** QUALCOMM MSM8916，固件 `MPSS.DPM.1.0.c7-00193-M8916EAAAANVZM-1 [Sep 09 2015]`
**SIM：** MY MAXIS，PLMN 50212，LTE，`registered` + `attached`
**libqmi：** 1.36.0

> 登录：`plink.exe -ssh -batch -pw 1313144 -hostkey SHA256:9/NFdvi+PH2k3/WI9nPDTLPX8bAR7/X3ULxwvGt/HOA -m <脚本> root@192.168.100.13`
> plink 用 `-m <文件>` 传脚本，不要用内联反引号转义（会被 PowerShell 吃掉）。

---

## 一、最重要的结论（推翻之前的判断）

**IMS bearer 必须跑在主口 `/dev/wwan0qmi0` + qmi-proxy 上，不是 DATA6。**

之前 memory 里"IMS 走 DATA6 独立端点"的结论**只对了一半**：
DATA6 上单发一条 `--wds-start-network` 确实能成功且不崩基带，
但 VoLTE 需要的是 **start-network → get-current-settings → 取 P-CSCF** 这一串多步流程，
而 **DATA6 上无法跨进程复用 CID**，第二步必然超时。

### 端点能力矩阵（实测）

| 能力 | `/dev/wwan0qmi0`（主口，rpmsg DATA5_CNTL） | `/dev/wwan0qmi1`（DATA6_CNTL，自编译模块） |
|---|---|---|
| QMI 服务齐全（wds/wda/uim…） | ✅ | ✅ `wds 1.36` `wda 1.11` |
| 链路格式 | raw-ip / no QoS header | raw-ip / no QoS header |
| 单发 `--wds-start-network` | ✅ | ✅ 返回 PDH，基带存活 |
| **跨进程复用 CID（关键）** | ✅ **可用**（需 qmi-proxy） | ❌ **`Transaction timed out`** |
| `--device-open-proxy` 生效 | ✅ | ❌（proxy 在跑也无效） |
| ModemManager 占用 | 主口，MM 用它 | MM 标为 `(ignored)`，udev 规则已生效 |
| `--wds-bind-data-port` | 未测 | ❌ `InvalidArgument` |
| `--wds-bind-mux-data-port` | 未测 | ❌ `InvalidQmiCommand`（2015 固件不支持） |

### 主口 CID 复用实测输出（决定性证据）

```
# STEP 1: 分配 CID
qmicli -d /dev/wwan0qmi0 --device-open-proxy --wds-noop --client-no-release-cid
→ Client ID not released: Service 'wds' CID '2'

# STEP 2: 复用同一 CID —— 成功
qmicli -d /dev/wwan0qmi0 --device-open-proxy --client-cid=2 \
       --client-no-release-cid --wds-get-packet-service-status
→ Connection status: 'disconnected'      exit=0   ← 真实应答，不是超时

# STEP 3: 同 CID 再查 settings —— 合法业务错误
→ error: couldn't get current settings: QMI protocol error (15): 'OutOfCall'
```

`OutOfCall` = "该 client 上没有活动会话"，这是**正确应答**（当时确实没起会话），
证明 transaction 通道完全正常。对比 DATA6 上同样操作永远是 `Transaction timed out`。

### 为什么 DATA6 不行

`rpmsg_wwan_ctrl_multi` 暴露的是一条**裸 rpmsg 管道**，不是 `/dev/cdc-wdm0` 那种
由内核做 QMUX 复用的设备。每次 `open()` 都是全新会话，上一次分配的 CID 不可寻址。
`qmi-proxy` 也救不了——实测 proxy 进程在跑（PID 确认）时复用依然超时。

---

## 二、崩溃机制（已复现两次）

**触发链：**

```
--wds-bind-data-port=N        → InvalidArgument（固件不支持）
  ↓ 客户端被污染
在同一 CID 上 --wds-start-network
  ↓
error: operation failed: endpoint hangup
  ↓
/dev/wwan0qmi1 消失，mmcli 报 "No modems were found"
  ↓
基带子系统重启（SSR）
```

**性质：** 是 **基带 SSR，不是内核 panic**。
`/sys/fs/pstore/` 为空；设备在 ~375s 时 remoteproc 自动重启、端口重新 attach、自愈。

但 **SSR 有时会升级成整机重启**：用户遇到的那次，我连上去时 `uptime` 只有 1 分钟，
而上一次启动的内核日志正好停在 `tun: Universal TUN/TAP device driver` ——
即 VoWiFi 起 TUN 的那一刻。

**放大器（已修复）：** 失败后重试逻辑会继续捶 —— 最多 5 次连接 + 3 次
`systemctl restart ModemManager`。对已 wedge 的基带反复下发 PDP 激活正是把
SSR 升级成设备失联的原因。已加 `FailureClass::BasebandWedged` 识别
`endpoint hangup` / `interface-in-use-config-match` / `MobileEquipment.Unknown`
并**立即中止整个重试批次**。

---

## 三、硬约束（写实现时必须遵守）

1. **绝不尝试任何 bind 命令**（`--wds-bind-data-port` / `--wds-bind-mux-data-port`）
   —— 直接触发 SSR。这块 2015 年固件不支持。
2. **DATA6 上绝不做多步流程** —— 只能单发一条命令。
3. **主口多步流程必须带 `--device-open-proxy`**，且 qmi-proxy 要在跑。
4. **打开辅助端点必须带 `--device-open-net='net-raw-ip|net-no-qos-header'`**
   —— 不带则 CID 分配报 `endpoint hangup`（`--wda-get-data-format` 确认端点确实是
   raw-ip / QoS header no）。
5. **这张 Maxis 卡 `ip-type=6` 会被网络拒绝**（`[3gpp] ipv4-only-allowed`），
   **必须先试 IPv4**。
6. **qmicli 一次只允许一个 WDS 动作**（多个会报 `too many WDS actions requested`）。

---

## 四、beta2（1.1.7）架构推断

从 `_strings_beta2.txt` 提取的关键字符串：

```
IMS allocated to primary qmi0; DATA6 is reserved for data
IMS allocated to DATA6; primary qmi0 is reserved for data
volte_data_slot_mode_missing
Native VoLTE secondary QMI IMS WDS bearer started
volte_secondary_qmi_wds_cid_missing
src/secondary_qmi_data.rs            ← 模块名是 secondary QMI *data*
Native VoLTE P-CSCF candidates discovered directly from QMI WDS
Native VoLTE QMI WDS CID is not numeric; skipping direct P-CSCF query
--wds-noop  --client-no-release-cid  --wds-set-ip-family=6
/run/qmi_auto_activate.ready
```

**推断：** beta2 有一个 **data slot mode**，可以在两种分配间切换，
而**能跑通的那种是 IMS 在主口、DATA6 让给数据**。
模块命名 `secondary_qmi_data.rs`（不是 `_ims.rs`）也印证了这一点。

`--wds-noop` 是它分配 CID 的方式（不带任何业务动作），然后复用该 CID。
这与本次实测"主口可复用 CID"完全吻合。

---

## 五、关于内核模块的定位（常见误解）

**`rpmsg_wwan_ctrl_multi.ko` 里没有任何 VoLTE 代码。**

它全部作用是给内核设备 ID 表加两行，让空闲的 rpmsg 通道变成设备节点：

```c
{ .name = "DATA6_CNTL", .driver_data = WWAN_PORT_QMI },   // -> /dev/wwan0qmi1
{ .name = "DATA7_CNTL", .driver_data = WWAN_PORT_QMI },   // -> /dev/wwan0qmi2
```

内核自带的 `rpmsg_wwan_ctrl` 只认 DATA1/DATA4/DATA5。
必须是内核模块的唯一原因：**创建字符设备节点、绑定 rpmsg 总线通道只能在内核态做**。

VoLTE 逻辑不能放进内核模块：内核里没有 socket/TLS/HTTP/SIP 栈；
内核崩溃 = 整机 panic（用户态崩了只是服务重启）；改一行就要按内核版本重编。

**修正后的角色：** 这个模块**仍然需要**，但用途变了 ——
不再是给 IMS 用，而是**给数据 bearer 用，把主口让给 IMS**。

---

## 六、当前设备状态（本次实测后）

- 内核模块已加载，`wwan0qmi1`(DATA6) / `wwan0qmi2`(DATA7) 均为 `type=QMI`
- MM 把 `wwan0qmi1` 标为 `(ignored)` —— udev 隔离规则生效
- `wwan0qmi2` 仍是 `(qmi)` —— udev 规则**没覆盖 DATA7**，是个小缺口
- bam-dmux 提供 `wwan0`–`wwan7` 八个 netdev（POINTOPOINT/NOARP，同一 remoteproc）
- qmi-proxy 已安装于 `/usr/libexec/qmi-proxy`（**不在 PATH**，`command -v` 查不到）
- SIM 有 `fixed-dialing` 锁 + `sim-pin2`（VoWiFi 能跑通，说明不影响 IMS 鉴权）
- AT 侧：CID 1 = UNET 内网 APN 已激活（10.0.73.236），CID 2 未激活

---

## 七、实现方案与落地状态（2026-07-27 本轮完成）

### 端口所有权修正（本轮真机 recon 新发现）

`/dev/wwan0qmi0` **不是 ModemManager 直接 open 的**——真正持有 fd 的是
`qmi-proxy`（本次实测 PID 17274），MM 只通过 proxy 的 socket 说话，自己直接
open 的只有两个 AT 口（`/dev/wwan0at0/at1`）。这正是"主口能跨进程复用 CID"的
底层机制：我们通过 `--device-open-proxy` 起的第二个 WDS client 与 MM 自己的
bearer 走同一条 proxy 通道，天然共存。因此原文"MM 独占主口"的说法要修正为
"qmi-proxy 独占主口，MM 与我们都是它的客户端"。

### 已实现的代码（编译 + 682 测试全绿）

| 模块 | 作用 |
|---|---|
| `cellular/qmi_wds.rs` | 主口 WDS 客户端：CID 分配/跨进程复用/set-ip-family/start/settings/stop；`WdsEndpoint` 区分主口(proxy, IMS 多步)与辅口(raw-ip flags, 单发)；辅口上跑多步流程直接被拒(`ImsFlowUnsupported`)；wedge 签名识别 + 永不重试；**编译期护栏测试**扫描源码禁止出现任何 `--wds-bind*` 参数 |
| `cellular/qmi_netdev.rs` | 会话建立后逐个 `wwanN` 网卡试探哪个承载数据：配置地址→发探测包(优先 DNS，回落网关)→看 rx 计数器；答复=`ProbeAnswered`，唯一候选=`SoleCandidate`，都不答=`Assumed`(标记未验证)；同基带配对，多基带不串线 |
| `access/volte/native_bearer.rs` | 把 WDS 会话适配成下游要的 `BearerConnection`；合成 `qmi-wds:` 前缀 path；`is_native_bearer` 让 teardown 区分 |
| `access/volte/live.rs` | `connect_inner` 里 native 路径优先于 MM 路径；session 持有 native handle；`cleanup_live_session` 按 bearer 类型分流释放 |

### 三条硬约束的落地

1. **DATA7 udev 缺口已补**：`main.rs` 的 `secondary-qmi-init` 改为用
   `discover_spare_qmi_ports()` 枚举**所有**空闲 QMI 口写 ignore 规则，不再只写
   被 prepare 的那一个。DATA6/DATA7 都会被 MM 忽略。
2. **DATA6 角色反转**：辅口只提供 `start_single_shot_session`（单发），多步 IMS
   流程物理上跑在主口。
3. **绝不 bind**：`qmi_wds.rs` 全程不构造 bind 参数，且有护栏测试锁死。

### 默认关闭，opt-in 上真机

native 路径由 `native_ims_bearer_enabled()`（env `SIMADMIN_VOLTE_NATIVE_IMS`）
控制，**默认走 MM 路径**。原因：`--wds-start-network=apn=ims` 在主口上的这一步
**尚未在参考基带上跑过**——前面每一步（proxy 就绪、CID 分配、跨进程复用、
set-ip-family、settings 读取）本轮都已真机验证，唯独 IMS 激活本身没验。这台
固件坏的 IMS 激活会重启基带，所以在真机确认前默认不启用。

### 本轮真机已验证（192.168.100.13，Maxis 50212，基带全程存活）

```
STEP1 主口 wds-noop 分配 CID          → CID '3'                    ✅
STEP2 跨进程复用同 CID (packet-status) → disconnected（真实应答）    ✅
STEP3 同 CID set-ip-family=4           → exit 0（原文未测过这步）    ✅
STEP4 同 CID current-settings          → OutOfCall（健康的空会话应答）✅
STEP5 释放 CID                         → exit 0                     ✅
辅口 wwan0qmi1 数据格式                → raw-ip / QoS header no      ✅
```

### 待真机验证（下一步）

- [ ] 打开 `SIMADMIN_VOLTE_NATIVE_IMS=1`，跑 `--wds-start-network=apn=ims,ip-type=4`
      主口激活是否成功且不崩基带（唯一未验证的一步）
- [ ] `qmi_netdev::resolve` 探测出的 `wwanN` 是否真的承载 IMS 会话数据
- [ ] P-CSCF 是否随 `current-settings` 的 PCO 返回

### 同时删除/新增的历史改动

- 删除 `access/volte/data_path.rs`（IPv6-only 的 WDS 预检，env 默认关闭、
  探测不存在的 `a2-mux-rmnet*`、从未生效）
- `FailureClass::BasebandWedged` + 重试批次立即中止（保留并复用于 native 路径）
