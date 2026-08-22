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

## 7. 与 SimAdmin 的关系

- 该崩溃在 SimAdmin 做任何 IMS 操作之前就已发生，不能归因于 SimAdmin。
- SimAdmin 侧**不应该**为此增加重试或放宽 `runtime_status=error` 的检查：内核会
  直接以 EINVAL 拒绝 OPEN，重试只会反复撞固件。当前的"拒绝并如实报错"是正确的。
- VoWiFi **不受影响**：它走 WiFi + 用户态 IKE/ESP/TUN，完全不碰 wwan 网卡。
  数据面卡死时 VoWiFi 仍可正常注册、收发短信与通话。
- 数据面恢复之后，VoLTE 的失败点会前移到 IMS 层
  （`ims_register_initial_receive_failed`、P-CSCF 可达性），那是另一个问题，
  见 `ue-isolation-migration.md` §8.7。
