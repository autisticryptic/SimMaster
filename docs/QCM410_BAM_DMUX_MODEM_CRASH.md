# QCM410 modem 固件冷启动崩溃与数据面卡死（`smd_dsm_memcpy.c:297`）

设备：410（jz02v10 4G Modem Stick，MSM8916），内核 `6.17.0-rc6` mainline，
modem 固件 `2022-11-05`。诊断时间 2026-08-22，地址 `192.168.100.13`。

本文档针对**反复出现**的那一类故障：ModemManager 能看到 modem、能注册网络、
短信和语音信令都正常，**唯独数据面完全用不了** —— VoLTE 起不来，数据代理拿不到
接口。与 `QCM410_BASEBAND_DISAPPEARANCE.md` 描述的"基带整个消失"是不同的两件事，
但根源同属这颗 Q6 modem 的固件崩溃。

---

## 1. 如何确认是这个问题

三条特征同时成立就是它：

```bash
# ① 内核日志里有开机时的固件 fatal
dmesg | grep 'fatal error received'
#   qcom-q6v5-mss 4080000.remoteproc: fatal error received: smd_dsm_memcpy.c:297:

# ② bam-dmux 的 runtime-PM 卡在 error
cat /sys/bus/platform/devices/4080000.remoteproc:bam-dmux/power/runtime_status
#   error

# ③ 所有 wwan 网卡继承该状态，且无法 OPEN
for i in 0 1 2 3 4 5 6 7; do
  echo "wwan$i $(cat /sys/class/net/wwan$i/device/power/runtime_status)"
done
ip link set dev wwan1 up
#   RTNETLINK answers: Invalid argument
```

SimAdmin 侧对应的报错是：

```
volte_bearer_netdev_runtime_error:interface=wwan0: runtime_status=error before OPEN
```

**这个报错是正确的行为，不是 SimAdmin 的 bug。** `ensure_bearer_interface_ready()`
在 `runtime_status=error` 时拒绝继续，是为了避免把真实故障掩盖成路由错误、并避免
再去撞固件。手工 `ip link set up` 同样返回 EINVAL，可以证明这一点。

---

## 2. 根因

冷启动时序（一次典型的坏启动）：

```text
12.59  powering up 4080000.remoteproc
12.65  MBA booted without debug policy, loading mpss
14.61  remote processor 4080000.remoteproc is now up      ← mpss 通过签名、正常启动
15.19  wwan wwan0: port wwan0at0 attached
15.37  wwan wwan0: port wwan0qmi0 attached
15.86  fatal error received: smd_dsm_memcpy.c:297         ← 起来 1.25 秒后崩
15.86  crash detected → recovering
17.51  remote processor is now up                         ← remoteproc 自己恢复了
```

要点：

- **MPSS 镜像本身没有问题。** 它通过了安全启动认证并成功运行；镜像损坏会在认证
  阶段失败，不会先 up 再崩。
- 崩溃点在 **DSM（Data Services Memory）**，即数据服务内存池 —— 这正好解释了
  "语音/短信信令正常、只有数据面死掉"。
- remoteproc 恢复了 modem，但 **Linux 侧 `bam-dmux` 驱动的 runtime-PM 被锁存在
  `error`**，重启固件并不会清除它。

**触发者是 mainline 的 `qcom_bam_dmux` 驱动 probe。** 固件是 2022 年的厂商版本，
内核是 2025 年的 mainline，而 `qcom_bam_dmux` 是社区重新实现的驱动，两者相隔三年 ——
典型的新驱动 × 旧固件启动竞态。

> **补充（2026-08-23）**：bam-dmux 的 probe 只是竞态的**一方**。整机重刷后，同样的
> 内核与固件不再崩溃，说明还需要另一方参与才会触发。最可能的另一方是 SimAdmin
> 安装器装的树外模块 `rpmsg_wwan_ctrl_multi.ko`，详见 §8。**§8 才是完整的因果与
> 预防办法**，本节只描述可观察到的现象。

> **重要（2026-08-23 晚）：这台设备上存在两种互不相同的固件崩溃。** 本文档到 §8
> 为止讲的都是第一种。第二种在同一天被发现并定位，见 **§10**：
>
> | | 第一种 | 第二种 |
> |---|---|---|
> | 固件断言 | `smd_dsm_memcpy.c:297` | `dhcp_client_mgr.c:263` |
> | 时刻 | 每次开机，boot 窗口内 | 运行中，绑定 DATA6 后 50–120 秒 |
> | 后果 | `bam-dmux` 锁存 `error`，需整机重刷 | 固件自恢复，`bam-dmux` 保持 `suspended` |
> | 根因 | 树外 `_multi` 模块抢绑 `DATA*_CNTL` | **SimAdmin 自己的无界重试循环** |
>
> 不要把 §9 的现象当成第一种的复发 —— 断言位置、时序和后果都不同，处理方式也不同。

---

## 3. 排查决策树（每一步排除一类原因）

按顺序做，每步的结论都能缩小范围。

| # | 实验 | 命令 | 观察到的结果 | 结论 |
|---|---|---|---|---|
| 1 | 手工拉起空闲网卡 | `ip link set dev wwan1 up` | `Invalid argument` | SimAdmin 的前置检查正确，不是过严 |
| 2 | 热重启 modem 子系统 | `echo stop > /sys/class/remoteproc/remoteproc0/state`，再 `start` | mpss 干净加载，**不崩** | **MPSS 镜像无损**，排除固件文件损坏 |
| 3 | 重新 probe 数据驱动 | `echo <dev> > /sys/bus/platform/drivers/bam-dmux/unbind`，再 `bind` | `error` → `suspended`，但 **netdev 全部消失**，报 `Timed out waiting for remote side to suspend` | `error` 是 **Linux 驱动侧锁存态**；modem 侧 DMUX 没起来 |
| 4 | 设备重启 | `reboot` | wwan 回来了，但同一崩溃复现 | **重启不能修复** |
| 5 | 恢复出厂 EFS | fastboot 刷 `modemst1`/`modemst2` | 崩溃照旧，**IMEI/校准完好** | **排除 EFS/NV 损坏**；该操作本身无害 |
| 6 | 拉黑数据驱动后冷启动 | `blacklist qcom_bam_dmux` + reboot | **完全不崩** | **probe 就是触发点** |
| 7 | 延迟加载 | modem 稳定后 `modprobe --ignore-install qcom_bam_dmux` | 不崩，8 个 netdev 建出，`wwan1` 能 UP | 延迟加载可规避竞态 |

第 2 步的具体命令（`remoteproc0`/`remoteproc1` 的编号每次启动可能对调，务必先按
名字确认）：

```bash
for rp in /sys/class/remoteproc/remoteproc*; do
  echo "$(basename $rp): $(cat $rp/name) $(cat $rp/state)"
done
# 认准 name == 4080000.remoteproc 的那个才是 modem
```

---

## 4. 明确无效的做法

省下时间，这些都试过了：

- **重启设备** —— 冷启动竞态每次都会复现。
- **重刷同一个系统镜像** —— 内核和固件都在镜像里，一模一样，崩溃照旧。
  （若上次"重刷后好了"，很可能是换了不同版本的镜像，或崩溃时刻恰好偏移。）
- **只重刷 `/lib/firmware` 里的基带文件** —— 同版本覆盖不改变任何东西；而且第 2 步
  已经证明镜像是好的。
- **恢复出厂 EFS** —— 已验证不能阻止崩溃（但无害，IMEI 与校准会保留）。
- **重启 ModemManager** —— 它只是上层观察者，改变不了内核/固件状态；而且它的 QMI
  探测本身也可能撞上同一个 DSM 故障。

---

## 5. 可行的缓解

### 5.1 崩溃时刻偏移就能自救

这是个竞态，所以**崩溃发生得足够晚**（晚于 bam-dmux 建立通道）时，驱动就能扛住：

```text
37.97  fatal error received: smd_dsm_memcpy.c:297   ← 晚于通道建立
       → bam-dmux runtime_status = suspended（健康）
       → 8 个 netdev 正常，wwan1 可以 UP
       → ModemManager: state = connected
```

实测恢复出厂 EFS 之后崩溃时刻从 t≈15.8s 推迟到 t≈38s，数据面因此可用。
**这说明任何拖慢 modem 早期数据服务初始化、或推迟 bam-dmux probe 的手段都可能奏效。**

### 5.2 延迟加载 `qcom_bam_dmux`（已验证可行）

```bash
# 1) 启动时不加载
printf 'blacklist qcom_bam_dmux\n' > /etc/modprobe.d/bam-dmux-defer.conf

# 2) 等 modem 稳定后再加载（做成 systemd unit，排在 ModemManager 之后并延时）
modprobe qcom_bam_dmux
```

实测结果：不触发崩溃，8 个 netdev 建出，`runtime_status=suspended`，
`ip link set wwan1 up` 成功。

**注意**：单纯拉黑会让 ModemManager 认不到 modem（进而影响需要 QMI UIM 的 VoWiFi），
所以必须配套"稍后再加载"，不能只拉黑。上线前需要把加载时机调稳。

---

## 6. 给厂商的材料

这是固件缺陷，最终要靠厂商修。报 bug 时附上：

1. 完整 `dmesg`，含 `fatal error received: smd_dsm_memcpy.c:297` 前后各 30 行；
2. modem 固件版本与日期（`ls -la /lib/firmware/mba.mbn /lib/firmware/modem.*`，
   本机为 2022-11-05）与内核版本（`uname -r`）；
3. 第 6、7 步的结论：**拉黑 `qcom_bam_dmux` 即可完全避免崩溃，延迟加载亦可** ——
   这直接指向固件在早期 DSM 初始化阶段对 BAM-DMUX 打开请求的处理；
4. modem coredump（若能抓到）：

```bash
echo enabled > /sys/class/remoteproc/remoteproc0/coredump   # 认准 4080000
# 崩溃后到 /sys/class/devcoredump/devcd*/data 取，5 分钟内会自动过期
```

注意冷启动崩溃发生在 t≈15s，早于用户态能设置 `coredump`，所以要抓它需要在
initramfs 或极早期的 systemd unit 里打开；热重启 modem 复现不了该崩溃，因此
**热重启抓不到这个 dump**。

---

## 7. 判断用户面通不通：两个常见误判

排查数据面时有两个坑，都踩过，单独列出来。

**坑一：只绑源地址，不绑接口。** 如果 WiFi 的默认路由 metric 比蜂窝低（本机
`wlan0` 600 vs `wwan0` 700），那么 `ping -I <地址>` 或 `socket.bind((src,0))`
发出的包会**带着蜂窝源地址从 WiFi 出去**，被当作非法源丢弃 —— 于是一个健康的蜂窝
数据面看起来像是全死的。必须绑接口：

```bash
ping -I wwan0 8.8.8.8                      # 接口名，不是地址
# python: s.setsockopt(SOL_SOCKET, SO_BINDTODEVICE, b"wwan0")
```

**坑二：`tx_packets` 计数器不可信。** bam-dmux 驱动**不更新** netdev 统计。实测
数据正常收发（ping 有回应、DNS 有应答）时：

```text
/sys/class/net/wwan0/statistics/tx_packets = 0
/sys/class/net/wwan0/statistics/rx_packets = 0
```

所以计数器为 0 **不能**作为"包没发出去"的证据。

**顺带一提**，用 `AF_PACKET`（tcpdump 同理）在 wwan 上抓到自己发的包，抓包点在
netdev 层、位于驱动交给硬件之前，同样只能证明包进了网卡。要判断链路通不通，
**唯一可靠的办法是做端到端往返测试**（绑接口的 ping / DNS / TCP），而不是看计数器
或抓包。

## 8. 为什么会崩：最可能的原因与预防（2026-08-23 更新）

整机重刷之后，**同一个内核、同一份 2022-11-05 固件，开机不再崩溃**
（`dmesg | grep -c 'fatal error received'` = 0，`bam-dmux runtime_status = suspended`，
8 个 wwan netdev 全部健康）。这一点很关键：它说明崩溃**不是内核或固件本身的确定性
缺陷**，而是取决于系统上积累的某种状态 —— 否则重刷同样的软件应该同样会崩。

### 8.1 最可能的原因：SimAdmin 自己装的树外内核模块

重刷后的干净系统与之前的差异，集中在 SimAdmin 安装器留下的**内核层**产物：

```text
重刷后（不崩）:  /lib/modules/<kver>/extra/        不存在
                 lsmod                              只有标准 rpmsg_wwan_ctrl
                 /etc/udev/rules.d/*simadmin*       无
                 simadmin-secondary-qmi.service     未安装

之前（每次开机崩）: deploy/install.sh 会安装并加载
                 kernel/rpmsg_wwan_ctrl_multi.ko   （树外内核模块）
                 99-simadmin-secondary-qmi.rules   （静态 udev 规则，已删除）
                 simadmin-secondary-qmi.service    （仍在，且是必需的）
```

`rpmsg_wwan_ctrl_multi` 是标准 `rpmsg_wwan_ctrl` 的自研替代/增强版，作用正是把
DATA5/DATA6 等**额外的 rpmsg 通道**暴露成多个 WWAN 控制口。而 `qcom_bam_dmux` 与
它挂在**同一条 `remoteproc0:smd-edge`** 上：

```text
remoteproc0:smd-edge.DATA5_CNTL / DATA6_CNTL / DATA7_CNTL / ... / DATA40_CNTL
```

推断的机制：冷启动时 modem 固件刚完成 mpss 加载、正在初始化 DSM（Data Services
Memory），此时 `_multi` 模块比标准模块**多绑定若干 DATA*_CNTL 通道**，这些额外的
通道建立请求与 `qcom_bam_dmux` 的 probe 在同一个狭窄窗口里并发打到固件上，把仍在
初始化的 DSM 撞崩 —— 于是 `smd_dsm_memcpy.c:297`。

这个推断能同时解释此前所有观察：

| 观察 | 是否吻合 |
|---|---|
| 拉黑 `qcom_bam_dmux` → 不崩 | ✅ 移走了竞争的一方 |
| 延迟加载 `qcom_bam_dmux` → 不崩 | ✅ 错开了那个窗口 |
| 热重启 modem → 不崩 | ✅ 通道已建立，无并发 bring-up |
| 恢复出厂 EFS → 仍崩，但崩溃时刻推迟 | ✅ 与 NV 无关，只是扰动了时序 |
| 整机重刷 → 完全不崩 | ✅ 移除了 `_multi` 模块 |

**注意这是推断而非证明**：崩溃前那台设备的模块加载状态已随重刷消失，无法回溯确认
`_multi` 当时确实是加载的。但它是唯一能解释"同样的内核与固件、重刷前崩重刷后不崩"
的差异项，且机制自洽。

#### 8.1.1 beta8 参考实现的佐证（2026-08-23）

从 `simadmin_1.1.7-beta8.tar.gz` 提取的证据把上面的推断坐实了一大半：

- beta8 的分发包**根本没有 `kernel/` 目录**，`system/` 下只有 udev 规则、
  secondary-qmi service 和 modem-recovery 三个文件 —— 它在**不带任何自研内核模块**
  的前提下做到了 DATA6 与 IMS 并存；
- 其 service 标题直接写着 `SimAdmin DATA6 stock RPMSG QMI initializer`，
  ExecCondition 用的是 **stock `rpmsg_wwan_ctrl`** + `driver_override` 绑
  `DATA6_CNTL`；
- 二进制里有一条日志：
  **`Migrated DATA6 runtime from the kernel-specific module to the stock RPMSG driver`**，
  并且伴随 `rmmod`、`/sys/module/rpmsg_wwan_ctrl_multi`、
  `/opt/simadmin/modules/rpmsg_wwan_ctrl_multi.ko`、`depmod -a`、
  `DATA6 legacy RPMSG driver did not detach` 等字符串 ——
  **beta8 会主动卸载并删除这个遗留模块**，而不只是解绑；
- 它还会写运行时规则 `/run/udev/rules.d/99-simadmin-secondary-qmi.rules`
  （内容形如 `SUBSYSTEM=="wwan", KERNEL=="wwan0qmi1", ENV{ID_MM_PORT_IGNORE}="1"`）
  把实际出现的那个端口对 ModemManager 隐藏，并检查
  `multiple WWAN ports appeared while binding DATA6` —— 即**只允许出现一个新端口**。

对照本项目：`secondary_qmi.rs` 已经迁到 stock 驱动、也会在候选被 legacy 驱动占用时
**解绑**它，但**从不卸载模块本身**。模块只要还加载着，它的 id_table 就会在每次开机
自动去绑其余 `DATA*_CNTL` 通道 —— 这正是撞崩 DSM 的那些额外绑定。

**已修复**（commit `f3308ed`）：新增 `purge_legacy_rpmsg_module()`，按 beta8 的做法
`rmmod` + 删除 `.ko` + `depmod -a`，并且**放在 DATA6 开关判断之前**执行 ——
`SIMADMIN_ENABLE_SECONDARY_QMI=0` 时旧代码会提前返回、完全不碰残留模块，而那恰恰是
模块纯属负担的配置。

udev 规则也已经对齐 beta8 的做法：静态的 `deploy/system/99-simadmin-secondary-qmi.rules`
已从仓库删除，规则改为在运行时按实际出现的端口名生成到 `/run/udev/rules.d/`。原先那条
静态规则匹配的是 `wwan[0-9]qmi1`/`qmi2`，而参考设备实际出现的端口叫 `wwan0at2` ——
它从来就没生效过，只是让人误以为端口已经对 ModemManager 隐藏了。

### 8.2 后续开发中如何避免

0. **升级到含 `f3308ed` 的版本**。`secondary-qmi-init` 现在会在每次启动时无条件
   卸载并删除遗留的 `rpmsg_wwan_ctrl_multi`，所以从旧版本升级上来的设备会自动清掉
   这个隐患，不需要人工处理。

1. **安装器已经不再碰内核层**（本轮修复）。`deploy/install.sh` 里那段
   "装 `.ko` / 从源码编译 / `modprobe rpmsg_wwan_ctrl_multi`" 的代码**已整段删除**。
   它此前是最危险的一处：即使 `secondary-qmi-init` 每次开机都把模块清掉，安装器
   仍会在安装结束时立刻 `modprobe` 把它加载回来 —— 自己的修复和自己的安装器对打。
   静态 udev 规则同样删掉了，ModemManager 的避让规则改由 `secondary-qmi-init`
   在运行时按**实际出现的端口名**生成到 `/run/udev/rules.d/`（详见
   `docs/INSTALL.md` 第 5 节）。现在要装的只有二进制、前端、carrier catalog、主
   systemd unit，以及需要 DATA6 时的 secondary-qmi unit 和 modem-recovery。

2. **内核模块永远不要作为默认安装项回归。** DATA6 走的是 in-tree
   `rpmsg_wwan_ctrl` + `driver_override`，这也是 beta8 在**完全没有树外代码**的
   前提下让 DATA6 与 IMS 并存的方式 —— 树外模块没有任何收益。如果将来某个平台
   真的需要它，必须是显式 opt-in 且与 DATA6 开关联动：开关关闭时既不装也不加载。

3. **升级/重装后做一次开机自检**：

```bash
dmesg | grep -c 'fatal error received'                                    # 应为 0
cat /sys/bus/platform/devices/4080000.remoteproc:bam-dmux/power/runtime_status  # 应为 suspended
lsmod | grep rpmsg                                                        # 不应出现 _multi（除非你确实要 DATA6）
```

### 8.3 万一又崩了：不必重刷

重刷代价大，而且**已经验证有更轻的办法**（§5.2）：拉黑 `qcom_bam_dmux`，等 modem
稳定后再加载。实测结果是不触发崩溃、8 个 netdev 正常、`ip link set wwan1 up` 成功。

排查顺序建议：

1. 先确认是不是这个故障（§1 的三条命令）；
2. 检查 `lsmod | grep rpmsg` 有没有 `_multi`；有就卸载它并从
   `/lib/modules/<kver>/extra/simadmin/` 移走，然后重启，看是否还崩；
3. 仍崩则用 §5.2 的延迟加载兜底；
4. 以上都无效，再考虑重刷。

## 9. 与 SimAdmin 的关系

- **第一种崩溃**（`smd_dsm_memcpy.c:297`）在 SimAdmin 做任何 IMS 操作之前就已发生，
  不能归因于 SimAdmin 的运行时行为 —— 但它的**成因**仍在 SimAdmin：是安装器装的树外
  模块（§8）。**第二种崩溃**（`dhcp_client_mgr.c:263`）则完全是 SimAdmin 自己造成的，
  见 §10。
- SimAdmin 侧**不应该**为第一种崩溃增加重试或放宽 `runtime_status=error` 的检查：内核
  会直接以 EINVAL 拒绝 OPEN，重试只会反复撞固件。当前的"拒绝并如实报错"是正确的。
  §10 是同一条原则在另一处被违反的后果。
- VoWiFi **不受影响**：它走 WiFi + 用户态 IKE/ESP/TUN，完全不碰 wwan 网卡。
  数据面卡死时 VoWiFi 仍可正常注册、收发短信与通话。
- 数据面恢复之后，VoLTE 的失败点会前移到 IMS 层
  （`ims_register_initial_receive_failed`、P-CSCF 可达性），那是另一个问题，
  见 `ue-isolation-migration.md` §8.7。

## 10. 第二种崩溃：`dhcp_client_mgr.c:263` —— SimAdmin 把固件打死的（2026-08-23）

### 10.1 现象

重刷后的干净系统、stock `rpmsg_wwan_ctrl`、`_multi` 模块确认未加载，DATA6 开启后
固件**每 50–120 秒崩一次**：

```text
[  109.717389] qcom-q6v5-mss 4080000.remoteproc: fatal error received: dhcp_client_mgr.c:263:
[  231.462449] qcom-q6v5-mss 4080000.remoteproc: fatal error received: dhcp_client_mgr.c:263:
[  285.368627] qcom-q6v5-mss 4080000.remoteproc: fatal error received: dhcp_client_mgr.c:263:
[  338.278098] qcom-q6v5-mss 4080000.remoteproc: fatal error received: dhcp_client_mgr.c:263:
```

与第一种不同：固件每次都**自行恢复**（`remote processor is now up`），
`bam-dmux` 保持 `suspended`，`Failed to resume` 计数为 0。

### 10.2 判定归因的那一步实验

**停掉 SimAdmin，崩溃立刻停止：**

```bash
systemctl stop simadmin.service simadmin-secondary-qmi.service
dmesg | grep -c 'fatal error received'   # 4
sleep 90
dmesg | grep -c 'fatal error received'   # 仍然是 4
```

固件闲置 90 秒零新增故障。**这条实验把归因钉死了：不是硬件、不是固件、不是驱动，
是 SimAdmin 在打它。**

### 10.3 因果链

1. 槽位分配器把 IMS 分给了 DATA6（`mode="independent_wwan1"`，
   `allocation="IMS allocated to DATA6; primary qmi0 is reserved for data"`），
   因为 ModemManager 已经在 qmi0 上持有数据承载（`primary_data_active=true`，
   见 `data_slot.rs:125`）；
2. 而 `secondary-qmi-init` 的职责之一就是**持续持有** `/dev/wwan0at2`
   （unit 描述里的 "initializer and **holder**"）；
3. 在被持有的设备上启动 WDS 会话必然失败，报文原样如下：

```text
[/dev/wwan0at2] Client ID not released:  Service: 'wds'  CID: '4'
[/dev/wwan0at2] couldn't detect transport type of port: unsupported wwan port
[/dev/wwan0at2] requested QMI mode but unexpected transport type found
error: couldn't start network: QMI protocol error (14): 'CallFailed'
verbose call end reason (2,201): [internal] error
```

4. `native_bearer.rs:253` 本来有护栏 —— `failure_class(&error).is_unsafe_to_retry()`，
   遇到楔死的基带就放弃整批尝试。但 `FailureClass::from_details` **在标点上漏掉了这个
   失败**：它匹配的是 `"call failed"` 和 `"internal error"`，而 qmicli 输出的是
   `'CallFailed'`（无空格）和 `[internal] error`（带方括号）。那套模式是照
   ModemManager 的措辞写的，原生 QMI 路径走的却是 qmicli；
5. 于是失败被归类成 `FailureClass::Other`（可重试），运行时无界重试
   ipv4 → ipv6 → attempt 1 → attempt 2 → …；
6. 每次重试都在被 holder 占着的设备上再开一次 WDS 会话。MSM8916 的 client 池很小，
   约一百秒就耗尽，固件的 DHCP 客户端管理器随之崩溃。

> **注意 `Client ID not released` 不是这条链上的一环。** 它读起来像"泄漏了一个
> client"，实际上是 qmicli 在确认我们自己传的 `--client-no-release-cid`（"Do not
> release the CID when exiting"）—— 见 §10.8。上面第 3 步的报文里有这一行，但真正
> 说明基带楔死的是 `[internal] error`，不是它。

### 10.4 已修复的部分

commit `9bb0913`：`is_baseband_wedge()` 现在同时匹配两种拼写（`'CallFailed'` /
`call failed`、`[internal] error` / `internal error`）。测试用的是设备上原样抓下来的
文本，同时钉住 ModemManager 那套拼写仍需两半同时命中（单独一个 `call failed` 或
`internal error` 太宽，不足以放弃整批）。

同一个 commit 还把 `client id not released` **单独**列为楔死信号，那部分是错的，已由
`330f059` 撤销 —— 见 §10.8。

### 10.5 已修复：分配策略固定为 IMS→qmi0、DATA6→数据

`secondary-qmi-init` 持有那个字符设备期间，**IMS-on-DATA6 根本不可能成功** —— 分配器
在选一个本设备无法履行的分配。当时列了两条路（让 holder 交接设备，或永不选它），
采用的是后者，因为它不需要引入交接协议这种新的失败模式。

commit `d081601`：**IMS 必定在 ModemManager 持有的端口（qmi0）上注册，DATA6 端口固定
分配给数据流量。** 这是本项目的不变量，不再从"哪个端点恰好忙"推导：

- `DataSlotMode::SecondaryImsPrimaryData` 这个变体被**整个删除**，而不是"尽量不选" ——
  一个绝不该被选中的枚举值留在类型里，只会等着下一个人再把它选出来；
- `ims_on_primary()` 恒为 `true`，于是 `native_ims_bearer_required()` 恒为 `false`，
  原生 QMI IMS 承载那条（撞 holder 的）路径不再被触发；
- `primary_data_active=true` 不再翻转 IMS，而是通过新增的
  `requires_primary_data_release()` 要求**把放错位置的数据从 qmi0 释放掉**，让它回到
  DATA6 —— 触发条件相同，但修的是错位的那个 bearer，而不是把 IMS 挪到一个被别人
  占着的端点上；
- `both_data_slots_active` 这个冲突也随之消失：在固定策略下，qmi0 上有普通数据不是
  "没有空闲端点"，而是"数据放错了地方"，是个可修复的前置条件。

实测（`d081601`，开机 9 分钟）：`fatal count` **0**、`mode="secondary_qmi_data"`、
VoLTE `IMS restore registered`、VoWiFi 200 OK、bearer 抖动 0、
`secondary-qmi` 端点丢失 0。对比修复前**每 50-120 秒崩一次**。

### 10.6 同一台设备上的第二场争用：数据网卡 `wwan1`

10.5 修好之后固件不再崩，但**数据依然不通**，而且失败方式会骗人：`wwan1` 上有一个
公网地址，看起来像通了。端到端测试才暴露真相：

```text
ping -I wwan1 8.8.8.8    → 100% packet loss
curl --interface wwan1   → 无输出
curl --interface wlan0   → 161.142.152.209    （WiFi 出口，对照用）
```

`mmcli` 给出了原因：

```text
Bearer/0  default-attach  APN=UNET   connected=yes
Bearer/2  default         APN=ims    connected=yes   interface=wwan1   ← 
```

`wwan1` 上那个地址**不是数据承载，是 ModemManager 自己建的 IMS 承载**（APN=`ims`）。
IMS APN 本来就不承载普通流量，所以 ping 不通是正常的 —— 而 SimAdmin 的 DATA6 数据
路径要用的也正是 `wwan1`。两边抢同一块网卡，先要到的赢：

```text
QMI protocol error (79): 'PolicyMismatch'      ← 手工 start-network 时的直接表现
```

**根因是 udev 规则的覆盖面不足。** 规则只隐藏了**控制口**：

```text
wwan0at2   SUBSYSTEM=wwan   (wwan_port)   ← 规则覆盖到了
wwan1      SUBSYSTEM=net    ID_MM_CANDIDATE=1   ← 完全没覆盖
```

两者是**不同子系统的两个独立 udev 设备**，所以 `SUBSYSTEM=="wwan"` 那条规则结构上
就不可能匹配到 netdev。而 ModemManager 给 `wwan1` 打了 `ID_MM_CANDIDATE=1`，明确
把它当候选口用。

commit `3136797`：`secondary-qmi-init` 现在为端点**同时**生成两条规则 —— 控制口一条
（`SUBSYSTEM=="wwan"`）、数据网卡一条（`SUBSYSTEM=="net"`），并且 `udevadm trigger`
要覆盖两个子系统（原先只 trigger `wwan`，新规则落在 `net` 上不会被重新应用）。
netdev 名字取自 `SecondaryQmiEndpoint.netdev`，即内核实际发布的那个，不猜名字。

硬件验证（重启 ModemManager 使其重新枚举）：

```text
wwan1 (ignored)                    ← MM 不再认领
Bearer/1 → interface: wwan0        ← MM 的承载挪回 wwan0
fault count: 0
```

> **时序要求**：`ID_MM_PORT_IGNORE` 必须在 ModemManager **枚举端口之前**就位。运行时
> 加规则再 `udevadm trigger` 会把属性写上，但已经在跑的 MM 不会重新枚举 —— 实测属性
> 落上了而 `wwan1` 仍是 `(net)`，直到重启 MM 才变成 `(ignored)`。单元已经是
> `Before=ModemManager.service`，所以正常启动路径没问题；但**手工改规则后必须重启
> ModemManager**，否则看到的是旧结论。

### 10.7 已验证：DATA6 数据面是通的（以及三次把它误判为不通）

`3136797` 冷启动实测：两条 udev 规则都由 `secondary-qmi-init` 运行时生成，
`wwan0at2 (ignored)`、`wwan1 (ignored)`，unit 重启次数 0，`fatal count` **0**，
VoLTE 421 → sec-agree → AKA → 200 OK。

**数据面结论：通。** 但要在正确的位置、用正确的方式测：

```bash
ip netns exec sa-ue<...> ping -I wwan1 8.8.8.8      # 4/4, 0% loss
ip netns exec sa-ue<...> curl --interface wwan1 ...  # 113.211.125.91  ← 蜂窝出口
ip netns exec sa-ue<...> curl ...                    # 161.142.152.209 ← WiFi（对照）
```

这一节主要记录**三个连续的测量错误**，因为每一个都足以让人得出"数据不通"的错误结论：

1. **在宿主命名空间里找 `wwan1`。** 它不在那里，而且**不该**在那里 ——
   `move_data_session_into_worker()` 会把网卡迁进 per-UE netns。宿主侧看到的
   `wwan1 oper=absent` 是迁移**成功**的表现，不是失败。
2. **用不绑定接口的 `curl` 测出口。** netns 里有两条默认路由，veth 那条
   （metric 0）故意优先于 `wwan1`（metric 500）—— 见 `secondary_qmi_data.rs:299`
   的注释：代理套接字用 `SO_BINDTODEVICE`，不靠默认路由取胜。所以未绑定的 `curl`
   走 WiFi 出口是**设计如此**，它测不出数据面通不通。
3. **看 `rx_packets`/`tx_packets` 判断有没有流量。** `bam-dmux` 在本平台不报
   per-netdev 计数：宿主 `wwan0` 在给 ModemManager 承载真实流量时同样是 0/0。

### 10.8 `Client ID not released` 不是泄漏 —— `9bb0913` 的过度匹配

§10.3 曾把这条消息读成"WDS client 泄漏"，§10.4 据此把它单独列为楔死信号。
**这个判断是错的。** qmicli 的 help 写得很直接：

```text
--client-no-release-cid    Do not release the CID when exiting
```

而 SimAdmin **每一次**secondary-QMI 调用都传这个 flag（`secondary_qmi_data.rs:402`、
`415`、`478`），CID 必须活过发起调用的那个进程。所以这条消息是 qmicli 在确认
"按你说的，我没释放" —— 一条 stderr 通知，被我们连同 stderr 一起并进了错误文本。
它出现在**每一条** secondary-QMI 失败里，与失败原因无关，因此不携带任何诊断信息。

后果：`is_baseband_wedge()` 匹配它，等于把所有 secondary-QMI 失败都判成
`BasebandWedged` → `is_unsafe_to_retry()`。VoLTE IMS 恢复循环
（`handlers.rs:8751`）因此会在一次**瞬态**失败后直接把线路打成
`Degraded`/`Exhausted` 并要求人工重试。

commit `330f059`：把 `client id not released` 从楔死集合中移除，并在 docstring 里
写明**不得再加回去**。区分两者靠的是 call-end reason，不是 CID 通知：

| | 崩溃签名（必须放弃） | PDN 争用（应当重试） |
|---|---|---|
| call end reason | `[internal] error` | `generic-unspecified` |
| CID 通知 | 有 | 有（无鉴别力） |
| 固件状态 | 楔死 | `fatal count` 0 |

两条都用设备上原样抓下来的文本钉了测试。§10.3 引用的崩溃期报文带
`(2,201): [internal] error`，仍然命中 `call_failed && internal_error`，护栏未被削弱。

**首次尝试失败的真正原因是 PDN 争用，不是楔死。** ModemManager 自己的承载在同一时刻
报同一个错：

```text
[modem0/bearer1] couldn't start IPv4 network: QMI protocol error (14): 'CallFailed'
```

MM 的 bearer 抖动时刻（22:01:11、22:07:16）与我们的失败时刻（22:01:11、22:07:17）
逐秒对应 —— MM 在 disconnect 后重跑 `simple connect`，两边同时向调制解调器要 PDN。
下一次尝试即成功（21:53:39 失败 → 21:54:51 成功，间隔 72 秒）。

> **一处需要更正的因果推断**：这 72 秒的间隔**不是**上述误分类造成的。数据面走
> `prepare_line_data_slot_for_volte` → `start_line_data_runtime_locked`，那条路径只
> `record_error`（`handlers.rs:3319-3327`），从不查 `FailureClass` —— 这也是日志里
> 两个 family 都被尝试过的原因。分类器修复对 VoLTE IMS 恢复循环是必要且正确的，
> 但它不是数据面那 72 秒的修复。恢复间隔由 watchdog 周期决定，仍是待办。

### 10.9 这次留下的通用教训

- **无界重试是可以把硬件打坏的**，不只是浪费时间。任何对基带的重试都必须有明确的
  "不可重试"分类，且该分类的**默认方向应当是保守的** —— 认不出来的失败应倾向于放弃，
  而不是倾向于重试。当前 `from_details` 的兜底是 `FailureClass::Other`（可重试），
  这个默认值本身值得重新考虑。
- **按错误文本字符串做分类是脆的。** 同一个失败经 ModemManager 和经 qmicli 出来的
  措辞不同，这次就差在一个空格和一对方括号上。新增一条产生错误的路径时，必须同时
  检查分类器认不认得它的措辞。
- **一个 QMI 端点是两个 udev 设备，不是一个。** 控制口在 `SUBSYSTEM=="wwan"`，
  数据网卡在 `SUBSYSTEM=="net"`。只隐藏控制口会留下另一半被 ModemManager 认领，
  而这一半才是承载流量的。凡是"为 SimAdmin 保留某个端点"的动作，都必须覆盖两者。
- **`ID_MM_PORT_IGNORE` 只在 ModemManager 枚举之前设置才有效。** 事后补规则 +
  `udevadm trigger` 会让属性出现在 `udevadm info` 里，却**不会**让 MM 放手 ——
  实测必须重启 MM 才生效。所以 `Before=ModemManager.service` 这个排序不是优化，
  是正确性要求。
- **"某个东西持有某个设备"应当是显式状态，而不是靠错误信息事后发现。** holder 与
  分配器此前对彼此一无所知，分配器因此会选一个注定失败的槽位。10.5 用固定策略
  绕开了这个问题，但底层的耦合仍未表达出来：将来若真要支持 IMS→DATA6，必须先有
  显式的持有权模型。
- **"接口有地址"不等于"数据通"。** `wwan1` 上那个公网地址曾让我误判数据已经打通，
  实际它属于 ModemManager 的 `ims` APN 承载，`ping -I wwan1` 100% 丢包。判定数据面
  必须端到端验证（`ping -I` / `curl --interface`），并核对**谁**建的承载、APN 是什么
  —— 见 §7 同类误判。
- **"接口不见了"同样不等于"数据不通"。** 与上一条方向相反、性质相同：per-UE netns
  隔离之后，宿主命名空间里**看不到**数据网卡才是正常的。测量之前必须先确定"该在哪个
  命名空间里看"，否则会把迁移成功读成迁移失败 —— 见 §10.7 第 1 条。
- **先读代码的意图，再判断观测结果是不是 bug。** netns 里 veth 默认路由优先于
  `wwan1`，这是 `secondary_qmi_data.rs:299` 注释里写明的设计（代理用
  `SO_BINDTODEVICE`）。我一度把它当成"蜂窝出口输掉了路由竞争"的缺陷。**观测到的行为
  与预期不符时，先去看那行代码有没有解释它。**
- **不要在诊断过程中动被诊断的对象。** 我用 `qmicli` 对 `/dev/wwan0at2` 做只读探测，
  两秒后 SimAdmin 的 bearer 就断了；那之后的"故障状态"是我自己造成的，不能作为证据。
  对活跃的 QMI 端口，任何手工调用都可能干扰会话 —— 观测应当只读日志与 `sysfs`。
- **"某个字符串出现在所有失败里"意味着它没有鉴别力，而不是它很重要。** 反过来说，
  给分类器加一个新签名之前，必须先确认它在**成功**路径上不出现。`Client ID not
  released` 就是这么进去的：它看起来触目，实际每次调用都有 —— 见 §10.8。
- **修复要对准被证实的因果链。** §10.8 的分类器修复是对的，但它修的是 VoLTE IMS 恢复
  循环，不是数据面那 72 秒。把两件事混为一谈会让人以为待办已经清掉。
