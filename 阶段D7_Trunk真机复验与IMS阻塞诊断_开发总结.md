# 阶段 D7：Trunk 真机复验与 IMS 阻塞诊断开发总结

> 日期：2026-07-17  
> 分支：`codex/ims-core-stage-a-b-live`  
> 关键检查点：`cf2c639 fix(volte): require bearer link activation`

## 一、代码修正

- 恢复 IMS raw-IP 网卡激活的严格失败语义：`ip link set dev <bearer-interface> up` 失败时立即返回。
- 不再忽略 `bam-dmux` 的 EINVAL/ETIMEDOUT 后继续配置地址和路由；Linux 在设备未 UP 时不会接受可用的下一跳路由，继续执行只会把真正的驱动/固件问题误报为后续 route 失败。
- 固定双栈策略保持不变：先请求 `ipv4v6`；网络明确要求单栈时直接尝试对应地址族，模糊失败时按 IPv4、IPv6 各一次有界回退。

## 二、质量门与 ARM64 构建

- `cargo fmt --check` 通过。
- bearer 定向测试 8/8、后端全量测试 545/545 通过；严格 Clippy `--all-targets --all-features -- -D warnings` 通过。
- ARM64 musl release 构建通过。
- 候选大小：9,515,992 bytes。
- SHA-256：`08A2929F63A0BDD04C5B45E0EF205FF9A042A2EC6D88BF32E04F4E03D6D2DFF3`。
- 启动日志准确标识提交 `cf2c639`。

## 三、Trunk 真机复验

- 隔离候选仅监听 `127.0.0.1:3101`，正式 `simadmin.service` 全程 inactive。
- 向真实 PJSIP 对端完成 Digest REGISTER 200；本地 SIP 端口 5062。
- 服务端实际有效期 3599 秒，计划刷新点 3059 秒；`register_attempts=2`，`reconnect_count=0`。
- API 正确暴露并持久化：
  - 注册时长 3600 秒；
  - 呼入类型 `bound_pending`；
  - 呼入绑定 6108；
  - 呼出绑定 6108；
  - IP 接通模式 `gsm_answer`。
- API 返回的 `secret` 为空且 `secret_set=true`，确认 Trunk 密码不会回显。

## 四、IMS bearer 受控诊断

1. 干净重启后双栈 IMS bearer 明确返回 `Ipv6OnlyAllowed`。
2. 删除失败 bearer 并创建纯 IPv6 bearer，仍返回 `Call failed: ipv6 error: prefix-unavailable`。
3. 按源码执行 AT P-CSCF 探测路径，`AT+CGACT=1,2` 同样失败，CID 2 未激活且没有 P-CSCF 数据。
4. `bam-dmux` 为 ARPHRD raw-IP（type 519）。当基带处于异常恢复窗口时，link-up 会超时或返回无效参数，并使 runtime-PM 进入 error；此时路由必然失败。
5. 历史成功检查点使用相同的“bearer 成功后严格 link-up”顺序，且曾在同机完成 IMS REGISTER、XFRM 和 VoLTE 短信。因此本轮不能通过忽略 link-up 来规避，当前阻塞仍是运营商 IPv6 prefix 分配与基带固件恢复状态。

## 五、能力边界

- Asterisk Trunk 注册、配置、双向 SIP 对话层、RTP relay 和 DTMF 离线层已就绪。
- 真实外呼/呼入必须先满足运营商 IMS `registered=true`；本轮未在 bearer 未连接时伪造拨号成功。
- `first_rtp` 与 `gsm_answer` 的 200 时机已有代码和离线测试，真机音频、CANCEL/BYE、RFC4733/SIP INFO 仍需在 IMS 恢复后验收。

## 六、设备清理结果

- 候选正常停止并释放 3101/5062，隔离目录及其中配置、数据库、日志和凭据全部删除。
- 正式 `simadmin.service` inactive，ModemManager active，modem 0 恢复驻网 46011。
- IMS bearer 列表为空；`wwan0` DOWN，runtime-PM 为 suspended；XFRM state/policy 为 0/0。
- CID 2 恢复为 `IPV4V6,""`，全部 `$QCPDPIMSCFGE` 项恢复为 `0,0,0`。
- Trunk/设备密码未写入 Git 或开发文档。

## 七、下一步

1. 等待运营商恢复 IMS IPv6 prefix 后，只做一次受控 bearer/REGISTER 验证。
2. IMS 注册成功后，从 Asterisk 分机发起外呼，依次验证 provisional、首 RTP 或 GSM 接通触发 200、双向 RTP、DTMF、CANCEL/BYE。
3. 再执行运营商呼入到绑定分机 6108，验证 `secondary_dial`、`bound_pending`、`bound_immediate` 三种呼入模式。
4. 音频稳定后继续 operator-originated re-INVITE 与 ViLTE 视频 relay。

## 八、2026-07-18 参考成品对照与结论修正

1. 在隔离 `/data`、仅监听 `127.0.0.1:3102` 的条件下运行参考成品 `1.1.6-dev18 / 05ea96a`。参考成品成功激活 IMS 上下文并发现 2 个 P-CSCF；对 `wwan1/a2-mux-rmnet1` 和共享 `wwan0/a2-mux-rmnet0` 的 SIP 能力探测虽均超时，但随后重新建立 ModemManager IPv6 IMS bearer 成功。
2. 最终 bearer 为 `/org/freedesktop/ModemManager1/Bearer/2`，APN `ims`、IP 类型 `ipv6`、接口 `wwan0`，获得公网 `/64` IPv6、网关和 DNS；参考成品安装 IPsec XFRM 后收到 REGISTER 200，运行态为 `registered=true`、`registration_mode=ipsec`。
3. 因此第四节“当前阻塞仍是运营商 IPv6 prefix 分配”的结论不再成立。直接 `mmcli/qmicli` 纯 IPv6 请求曾返回 `prefix-unavailable`，但参考成品完整初始化流程在同一设备、同一网络下可以成功；后续应聚焦其前置 IMS 上下文、数据路径探测、ModemManager 重枚举等待和 bearer 创建时序。
4. 参考成品停止后进行了 A/B bearer 复测。成品完成一次注册后，即使暂时把 CID 2 的 `$QCPDPIMSCFGE` 清零，新的纯 IPv6 bearer 仍可连接；说明成功不能只归因于单个 AT 标志，基带状态和完整初始化顺序共同参与。
5. 当前源码候选启动时会重启 ModemManager，实测 modem 对象可从 `Modem/0` 重枚举为 `Modem/1`。旧全局 VoLTE 路径仍固定使用 modem `0`，导致 `volte_command_failed:mmcli:1`。源码已改为：先验证保存的 modem ID，失效时从 `mmcli -L --output-keyvalue` 的 `modem-list.value[1]` 解析当前 ID，并保留原 QMI 设备与 UIM slot。
6. 同时增强 VoLTE 的 `mmcli/ip` 失败详情，记录退出码、无凭据命令参数和 stderr，便于下一次直接定位具体失败步骤；不记录 SIM、AKA、Trunk 或设备密码。
7. 质量门：全量 Rust 测试 546/546 通过；新增 modem 重枚举解析测试通过；VoLTE 定向测试 121/121 通过；严格 Clippy 通过。
8. 修复候选第二次真机运行仍出现一次泛化 `mmcli:1`，随后设备 SSH 与网络不可达，无法在本轮完成增强诊断版本部署和设备清理确认。该状态必须在设备恢复上线后优先审计；不得把本轮记录为当前源码已完成 IMS 注册。
9. 修复提交为 `f50d7de fix(volte): follow modem reenumeration`。对应 ARM64 musl 候选大小 9,518,672 bytes，SHA-256 为 `33DF08DB06B1026649EE065BD29ED654CE63F9F1D95F605DD2A4223A62A33D3D`；设备恢复后已上传并完成下一轮诊断。
10. 设备重启恢复后已部署 `f50d7de` 并得到精确错误：AT P-CSCF 探测期间 modem 0 消失、operating mode 进入 `shutting-down`，随后纯 IPv6 `--create-bearer` 失败为 `error: couldn't find modem`。因此修复继续收敛为两点：P-CSCF 固定 IPv6 优先（与参考成品相同），且 AT 探测后、创建 bearer 前再次等待 ModemManager 恢复到 `registered/connected` 并重新解析当前 modem ID。新增 VoLTE 定向测试后为 122/122，严格 Clippy 通过。

修正后的下一步：

1. 设备恢复后先检查并停止遗留的 3103 隔离进程，删除 `/opt/simadmin/releases/cf2c639-live` 与临时 `/data`，恢复 modem online、空 bearer、`wwan0/wwan1` DOWN、XFRM 0/0、CID 2 和全部 `$QCPDPIMSCFGE` 基线。
2. 部署包含 modem 重枚举和详细命令错误的最新提交，复验当前源码的 AT 探测、IPv6 bearer、P-CSCF、XFRM 与 REGISTER。
3. 当前源码成功 `registered=true` 后，再继续原第 2–4 项 Trunk 呼叫、RTP、DTMF 与 ViLTE 验收。
