# IMS 接入路径重构：410 实机回归清单

> 410 已于 2026-08-22 在 `192.168.100.13` 上线并部署提交 `48e7e41` 的
> GitHub Actions ARM64 产物。当前已确认 per-UE namespace、worker、veth、NAT
> 基础设施能够启动，且 VoWiFi 的 IKEv2/EAP-AKA/CHILD_SA/ESP/TUN socket 已在
> worker netns 内创建。只读回归还确认飞行模式下 3GPP/VoLTE 为 down，VoWiFi 的
> ePDG/IKE/CHILD_SA/ESP/TUN 保持 ready；当前 IMS REGISTER 仍收到运营商 `421`
> 拒绝，因此短信、语音等业务级项目仍需按功能门分阶段验证，未勾选项目不得视为通过。

## 本轮变更

- IMS registration、3GPP access、non-3GPP access、voice access selection 分层建模。
- VoLTE 与 VoWiFi 可同时注册；语音选择独立于注册状态。
- 飞行模式关闭蜂窝 radio、VoLTE 和 cellular data，但不阻断 VoWiFi restore，也不主动拆除健康的 VoWiFi registration。
- radio 或 ePDG/IKE/IPsec 已断时，陈旧 snapshot 不再显示 registered。
- 概述页通过 `/api/modem/lines/{line_id}/ims/status` 展示分层状态，reader 不再固定为“不适用”。

## 410 分阶段必测

### 测试设备映射

- 410 实机：`192.168.100.13`，ADB serial `0123456789`；部署登录密码由设备环境提供。
- 卡一拨号/媒体测试设备：ADB serial `98d45ad8`。
- 本轮仅使用 410 做实机回归；EC20/EC25/EG25/EG600 与 USB/PCSC reader 保持待测。

### 飞行模式与 VoWiFi

- [ ] 开启飞行模式后 data 停止、3GPP/VoLTE 为 `down`，不继续显示 registered。
- [ ] 开启飞行模式时，已健康注册的 VoWiFi 不因 3GPP 关闭而被无条件 teardown；失效连接仍可自动修复。
- [ ] 飞行模式下 VoWiFi 可建立 ePDG、IKEv2/IPsec、P-CSCF 和 IMS 注册。
- [ ] 飞行模式下短信经 VoWiFi 接收，不误走 CS。
- [ ] 关闭飞行模式后 3GPP 独立恢复，不销毁健康的 VoWiFi registration。

### 两条 access 共存与语音选择

- [ ] 同时启用 VoLTE/VoWiFi，两路独立完成 bearer/tunnel、P-CSCF 与 REGISTER。
- [ ] `registration.registered_over` 同时包含 `vowifi` 和 `volte`。
- [ ] 默认策略 `voice.active` 选择 VoWiFi，同时保留 VoLTE fallback。
- [ ] 断 Wi-Fi 只清除 non-3GPP，语音退回 VoLTE；Wi-Fi 恢复后 VoLTE 不被销毁。
- [ ] access 失败不会清空另一条 access 的 P-CSCF、注册或媒体状态。

### P-CSCF、多线路和隔离

- [ ] 两条 access 展示各自 P-CSCF，不能串用。
- [ ] 多基带或基带加 reader 的操作均绑定正确 `line_id`/UE context。
- [ ] 相同 IP、网关、P-CSCF 时 netns、路由、XFRM、SIP、RTP 仍互不干扰。
- [ ] worker 异常退出后可恢复，且不遗留 namespace、route 或 XFRM。
- [ ] 按 `docs/ue-isolation-migration.md` 验证数据代理与 Trunk 的 per-UE 映射。
- [ ] ModemManager 主接口 `wwan0` 不被迁移；只迁移 SimAdmin 本次 native bearer
  创建的非 `wwan0` 接口。
- [ ] native `wwanN` 迁移失败、worker 网络配置失败或 worker 消失时，接口回宿主并
  释放 QMI/WDS session，不遗留半连接 bearer。
- [ ] P-CSCF 的 PCO、AT active-context 和 worker DNS fallback 均可观测并能分别验证。
- [ ] 本次 IMS XFRM 只在当前 worker 安装/删除，不执行宿主或其他线路全局 flush。

注意：VoLTE `wwanX` 完整迁入 worker/netns 仍需逐步实机回归，不得仅凭 VoWiFi worker 测试宣称完成。

### 短信

- [ ] 分别验证 CS、VoLTE IMS、VoWiFi IMS 接收短信。
- [ ] 多接收面观测同一短信时只入库一次，并保留真实路径证据。
- [ ] 飞行模式下不从 CS 接收。
- [ ] 前端与通知渠道收到同一规范化短信事件，不因线路数增加而重复轮询。

### 语音、媒体与 DTMF

- [ ] VoWiFi/VoLTE 来电接听、双向 RTP、DTMF、挂断同步。
- [ ] RTP 入出站网口与所选 UE 一致，不串线路。
- [ ] 待机 access 切换不修改已建立 dialog；通话固定在建立时的 leg。
- [ ] 视频来电的语音降级、视频协商、挂断同步分别记录。
- [ ] `trunk_sockets_in_worker` 开启时 operator RTP 在当前线路 netns；Asterisk/internal
  leg 保持宿主网络，二者桥接后仍可双向收发。

### 数据代理

- [ ] DATA6/secondary bearer 迁入对应线路 worker，`wwan0` 不受影响。
- [ ] HTTP、HTTP CONNECT、SOCKS5 均从所选线路出口，不能回落到 Wi-Fi/其他基带。
- [ ] 两条线路获得完全相同地址时，代理连接和流量计数仍分别归属正确线路。
- [ ] 停止线路后 worker 路由/地址清理，secondary interface 回宿主 namespace。

当前阶段不承诺通话中的无缝 VoLTE/VoWiFi handover、IMS service continuity 或 SRVCC。

### UI/API 与竞态

- [ ] 概述页显示 `VoWiFi`、`VoLTE`、`VoWiFi + VoLTE`、`连接中`、`异常`、`未注册`、`状态未知`。
- [ ] reader 具备 VoWiFi runtime 时显示真实 IMS 状态。
- [ ] API 的 `registration`、`three_gpp`、`non_three_gpp`、`voice` 语义独立。
- [ ] 飞行模式快速切换不因旧 snapshot 继续显示 VoLTE registered。
- [ ] Wi-Fi/IPsec 拆除后不因旧 snapshot 继续显示 VoWiFi registered。
- [ ] 连续开关不会重复 restore、旧任务覆盖新状态或无限重试。

## 建议诊断采集

每个测试场景至少保留以下证据，并确保记录中包含 `line_id`：

```bash
curl -s -b /tmp/simadmin.cookies http://127.0.0.1:3000/api/modem/lines/<line_id>/ims/status
journalctl -u simadmin --since "10 minutes ago" --no-pager
ip -br link
ip route show table all
ip rule show
ip netns list
```

涉及 VoWiFi 时另采集 IKE/XFRM 与 worker namespace；涉及 VoLTE 时另采集对应 `wwanX`、QMI bearer、P-CSCF、SIP REGISTER 和 RTP 绑定信息。若设备实际 API 监听端口不同，以部署配置为准。

## 当前实机基线与暂不执行项

- 当前 410 线路为 `line-50ad5391cd09c09936f1081bd479139c`，已建立
  `sa-ue286e0c9d2870` namespace、veth `/30` 地址和宿主 NAT；这些只证明隔离底座
  已启动，不等于 VoWiFi、VoLTE、数据代理或 Trunk 业务验收完成。
- 当前 VoWiFi 已完成 ePDG、IKE、CHILD_SA、ESP 与 TUN，但 IMS REGISTER 收到
  SIP `421`，尚未进入 `voice_ready`；应先排除此注册阻碍，再执行真实短信/语音测试。
- 本轮实机确认主进程创建的 IKE socket 位于 worker 的 netns，worker 内可见
  `lo`、`save<hex>` 与 `sa_vwf<hex>`，宿主没有对应的 500/4500 socket；飞行模式下
  3GPP 状态为 `down`，没有继续显示 VoLTE 已注册。
- SIP `421` 在迁移前的旧部署也存在，当前证据不足以归因于 netns/veth 迁移；后续应
  单独比较宿主路径与 worker 路径的 REGISTER 报文和运营商 profile，再修复业务注册。
- 当前 VoLTE、数据代理与 Trunk worker 功能门仍需逐项启用并回归，首次验证不得同时
  打开多个功能门；本次提交完成后应先重新验证 worker/namespace，再进入业务测试。
- 线路刷新已改为先准备 UE worker/socket 状态、再与 binding 一起发布；DATA6 保留会话
  会检查 worker 实例和 netdev 可见性。以上只证明代码路径具备失效保护，仍需在 410
  上验证 worker 重建、secondary bearer 回宿主和重新建立的完整过程。
- 不在本机生成 production 构建或发布包；发布构建继续交给 GitHub Actions。
- 暂不执行 EC20、EC25、EG25、EG600 与 PCSC/USB SIM reader 实机测试。
- 暂不承诺通话中的无缝 access handover、IMS service continuity 或 SRVCC。
