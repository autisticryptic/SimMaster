# SimAdmin API Collection - Bruno

这是用于 [Bruno](https://www.usebruno.com/) 的 API 测试集合，包含了 SimAdmin Backend 的 API 接口。

## 文件列表

### 基础信息接口
- **get_device_info.bru** - 获取设备信息（IMEI、制造商、型号、在线状态）
- **get_sim_info.bru** - 获取 SIM 卡信息（ICCID、IMSI、手机号、MCC/MNC 等）
- **get_health.bru** - 健康检查

### 网络相关接口
- **get_network_info.bru** - 获取网络信息（运营商、注册状态等）
- **get_cells_info.bru** - 获取小区信息（主小区+邻区）

### 数据连接接口
- **get_data_status.bru** - 获取数据连接状态
- **set_data_status_enable.bru** - 启用数据连接
- **set_data_status_disable.bru** - 禁用数据连接
- **get_roaming_status.bru** - 获取漫游状态（是否允许漫游、当前是否漫游）
- **set_roaming_enable.bru** - 启用漫游数据
- **set_roaming_disable.bru** - 禁用漫游数据

**注意**：切换数据连接状态不会清空宿主机 iptables/ip6tables 规则，避免影响 Docker、VPN、防火墙等宿主机网络配置。

**漫游说明**：插入境外 SIM 卡时，如果网络注册状态为 `roaming`，需要启用漫游开关才能使用数据连接。

### 飞行模式接口
- **get_airplane_mode.bru** - 获取飞行模式状态
- **set_airplane_mode_enable.bru** - 启用飞行模式（关闭射频）
- **set_airplane_mode_disable.bru** - 禁用飞行模式（开启射频）

### 系统统计接口
- **get_stats.bru** - 获取综合系统统计（网速+内存+CPU+运行时间+温度+USB模式）
- **get_cpu_info.bru** - 获取CPU详细信息

### 定位相关接口
- **get_cell_location_info.bru** - 获取基站定位参数（MCC/MNC/LAC/CID）

### 网络接口详情
- **get_network_interfaces.bru** - 获取所有网络接口详情（IP/MAC/流量统计）

### 射频模式接口（4G/5G 切换）
- **get_radio_mode.bru** - 获取当前射频模式
- **set_radio_mode_auto.bru** - 设置为 4G/5G 自动模式
- **set_radio_mode_lte.bru** - 设置为仅 4G LTE 模式
- **set_radio_mode_nr.bru** - 设置为仅 5G NR 模式

### 频段锁定接口
- **get_band_lock.bru** - 获取当前频段锁定状态
- **set_band_lock_lte_b1_b3.bru** - 锁定 LTE B1+B3（示例）
- **set_band_lock_nr_n78.bru** - 锁定 NR N78（示例）
- **set_band_lock_lte_nr_mix.bru** - 混合锁定 LTE 和 NR 频段（示例）
- **unlock_all_bands.bru** - 解除所有频段锁定

### 系统控制接口
- **post_system_reboot.bru** - 系统重启（可设置延迟秒数）

### OTA 更新接口
- **get_ota_status.bru** - 获取 OTA 更新状态（当前版本、待安装更新）
- **post_ota_apply.bru** - 应用 OTA 更新（不重启）
- **post_ota_apply_restart.bru** - 应用 OTA 更新并立即重启
- **post_ota_cancel.bru** - 取消待安装的 OTA 更新

**OTA 更新说明**：
- 支持 `.tar.gz` 和 `.zip` 两种格式（推荐 tar.gz，能保留 Linux 文件权限）
- 上传大小限制：50MB
- OTA 包结构：`meta.json`（元数据） + `simadmin`（二进制） + `www/`（前端）
- 自动验证：二进制 MD5、架构匹配、版本号比较
- ZIP 格式会自动修复文件权限（二进制 755，前端文件 644）
- 上传接口 `/api/ota/upload` 需使用 `multipart/form-data` 或 `application/octet-stream`

### 电话功能接口
- **get_calls.bru** - 获取当前通话列表
- **post_call_dial.bru** - 拨打电话
- **post_call_hangup.bru** - 挂断指定通话
- **post_call_hangup_all.bru** - 挂断所有通话
- **post_call_answer.bru** - 接听来电

### 短信功能接口
- **post_sms_send.bru** - 发送短信
- **get_sms_list.bru** - 获取短信列表（分页）
- **get_sms_stats.bru** - 获取短信统计
- **post_sms_clear.bru** - 清空所有短信历史

### 新增功能接口
- **get_signal_strength.bru** - 获取信号强度详细信息
- **get_ims_status.bru** - 获取 IMS（VoLTE）状态
- **get_call_volume.bru** - 获取通话音量设置
- **set_call_volume.bru** - 设置通话音量
- **get_voicemail_status.bru** - 获取语音留言状态
- **get_operators.bru** - 获取当前运营商
- **scan_operators.bru** - 扫描所有可用运营商（慢，120秒）
- **register_operator_manual.bru** - 手动注册到指定运营商
- **register_operator_auto.bru** - 自动注册运营商
- **get_call_forwarding.bru** - 获取呼叫转移设置
- **set_call_forwarding.bru** - 设置呼叫转移
- **get_call_settings.bru** - 获取通话设置
- **set_call_settings.bru** - 设置通话设置

### APN 管理接口
- **get_apn_list.bru** - 获取 APN 配置列表
- **set_apn.bru** - 设置 APN 配置

### 设备网络接口
- **get_device_ddns_config.bru** - 获取 DDNS 配置
- **set_device_ddns_config.bru** - 保存 DDNS 配置
- **get_device_ddns_status.bru** - 获取 DDNS 状态
- **post_device_ddns_sync.bru** - 立即执行 DDNS 同步
- **get_device_ddns_logs.bru** - 获取 DDNS 同步日志
- **post_device_ddns_logs_clear.bru** - 清空 DDNS 同步日志
- **get_device_wlan_status.bru** - 获取 WLAN 状态
- **post_device_wlan_scan.bru** - 扫描 WLAN 热点
- **get_device_wlan_profiles.bru** - 获取已保存 WLAN 网络
- **post_device_wlan_forget.bru** - 忘记已保存 WLAN 网络
- **post_device_wlan_connect.bru** - 连接 WLAN 热点
- **post_device_wlan_disconnect.bru** - 断开 WLAN
- **post_device_wlan_profile.bru** - 保存 WLAN 配置
- **set_device_wlan_enabled.bru** - 开关 WLAN

### 通话记录接口
- **get_call_history.bru** - 获取通话记录列表（分页）
- **delete_call_history.bru** - 删除单条通话记录
- **clear_call_history.bru** - 清空所有通话记录

### 通知配置接口
- **get_notification_config.bru** - 获取多渠道通知配置
- **set_notification_config.bru** - 设置多渠道通知配置
- **test_notification_channel.bru** - 测试指定通知渠道，可通过 `channel` 变量切换

### WiFi Calling (VoWiFi) 接口
- **get_vowifi_status.bru** - 获取 VoWiFi 连接状态
- **get_vowifi_control.bru** - 获取 VoWiFi 控制配置
- **set_vowifi_feature.bru** - 开启或关闭 VoWiFi 功能（控制 UI 和功能的激活）
- **set_vowifi_connection.bru** - 开启或关闭 VoWiFi 连接（控制后台隧道的拨号）
- **post_vowifi_connect.bru** - 手动触发 VoWiFi 立即连接与重试
- **get_vowifi_profile.bru** - 获取当前匹配的 VoWiFi 运营商配置详情
- **get_vowifi_profiles.bru** - 获取系统支持的所有预设 VoWiFi 运营商配置
- **get_vowifi_diagnostics.bru** - 获取 VoWiFi 诊断详细数据与时序监控数据
- **get_vowifi_events.bru** - 获取 VoWiFi 运行日志流
- **get_vowifi_soak.bru** - 获取 VoWiFi 稳定性压力测试运行记录
- **get_vowifi_sms_delivery.bru** - 获取基于 IPsec 隧道的短信投递记录列表
- **get_vowifi_esim_restore_status.bru** - 获取 eSIM 切卡与基带状态恢复的同步进度

## 使用方法

1. **安装 Bruno**
   - 访问 https://www.usebruno.com/ 下载安装
   - 或使用 `brew install bruno` (macOS)

2. **打开集合**
   - 在 Bruno 中点击 "Open Collection"
   - 选择 `bruno-api` 文件夹

3. **修改 IP 地址**
   - 所有请求默认使用 `http://192.168.68.1:3000`
   - 如需修改，可在 Bruno 中批量替换或使用环境变量

4. **发送请求**
   - 点击任意 `.bru` 文件
   - 点击 "Send" 按钮发送请求

## API 端点说明

| 方法 | 端点 | 说明 |
|------|------|------|
| GET | `/api/health` | 健康检查 |
| GET | `/api/device` | 设备信息（IMEI/ICCID/IMSI） |
| GET | `/api/sim` | SIM 卡信息 |
| GET | `/api/network` | 网络信息 |
| GET | `/api/cells` | 小区信息 |
| GET | `/api/data` | 数据连接状态 |
| POST | `/api/data` | 设置数据连接 |
| GET | `/api/roaming` | 漫游状态（是否允许、是否漫游中） |
| POST | `/api/roaming` | 设置漫游开关 |
| GET | `/api/airplane-mode` | 飞行模式状态 |
| POST | `/api/airplane-mode` | 设置飞行模式 |
| GET | `/api/stats` | 综合系统统计（网速+内存+运行时间+系统信息） |
| GET | `/api/stats/cpu` | CPU信息 |
| GET | `/api/location/cell-info` | 基站定位参数 |
| GET | `/api/network/interfaces` | 网络接口详情 |
| GET | `/api/radio-mode` | 射频模式（Auto/LTE/NR） |
| POST | `/api/radio-mode` | 设置射频模式 |
| GET | `/api/band-lock` | 频段锁定状态 |
| POST | `/api/band-lock` | 设置频段锁定 |
| POST | `/api/system/reboot` | 系统重启 |
| GET | `/api/ota/status` | 获取 OTA 更新状态 |
| POST | `/api/ota/upload` | 上传 OTA 更新包（50MB 限制） |
| POST | `/api/ota/apply` | 应用 OTA 更新 |
| POST | `/api/ota/cancel` | 取消待安装的更新 |
| GET | `/api/calls` | 获取当前通话列表 |
| POST | `/api/call/dial` | 拨打电话 |
| POST | `/api/call/hangup` | 挂断指定通话 |
| POST | `/api/call/hangup-all` | 挂断所有通话 |
| POST | `/api/call/answer` | 接听来电 |
| POST | `/api/sms/send` | 发送短信 |
| GET | `/api/sms/list` | 获取短信列表（分页） |
| GET | `/api/sms/conversation` | 获取与指定号码的对话 |
| GET | `/api/sms/stats` | 获取短信统计 |
| POST | `/api/sms/clear` | 清空所有短信 |
| GET | `/api/network/signal-strength` | 获取信号强度详细信息 |
| GET | `/api/ims/status` | 获取 IMS（VoLTE）状态 |
| GET | `/api/call/volume` | 获取通话音量设置 |
| POST | `/api/call/volume` | 设置通话音量 |
| GET | `/api/voicemail/status` | 获取语音留言状态 |
| GET | `/api/network/operators` | 获取当前运营商 |
| GET | `/api/network/operators/scan` | 扫描所有运营商（慢） |
| POST | `/api/network/register-manual` | 手动注册运营商 |
| POST | `/api/network/register-auto` | 自动注册运营商 |
| GET | `/api/call/forwarding` | 获取呼叫转移设置 |
| POST | `/api/call/forwarding` | 设置呼叫转移 |
| GET | `/api/call/settings` | 获取通话设置 |
| POST | `/api/call/settings` | 设置通话设置 |
| GET | `/api/apn` | 获取 APN 配置列表 |
| POST | `/api/apn` | 设置 APN 配置 |
| GET | `/api/device-network/ddns/config` | 获取 DDNS 配置 |
| POST | `/api/device-network/ddns/config` | 保存 DDNS 配置 |
| GET | `/api/device-network/ddns/status` | 获取 DDNS 状态 |
| POST | `/api/device-network/ddns/sync` | 立即执行 DDNS 同步 |
| GET | `/api/device-network/ddns/logs` | 获取 DDNS 同步日志 |
| POST | `/api/device-network/ddns/logs/clear` | 清空 DDNS 同步日志 |
| GET | `/api/device-network/wlan/profiles` | 获取已保存 WLAN 网络 |
| POST | `/api/device-network/wlan/forget` | 忘记已保存 WLAN 网络 |
| GET | `/api/device-network/wlan/status` | 获取 WLAN 状态 |
| POST | `/api/device-network/wlan/enabled` | 开关 WLAN |
| POST | `/api/device-network/wlan/scan` | 扫描 WLAN 热点 |
| POST | `/api/device-network/wlan/connect` | 连接 WLAN 热点 |
| POST | `/api/device-network/wlan/disconnect` | 断开 WLAN |
| POST | `/api/device-network/wlan/profile` | 保存 WLAN 配置 |
| GET | `/api/call/history` | 获取通话记录列表 |
| DELETE | `/api/call/history/:id` | 删除单条通话记录 |
| POST | `/api/call/history/clear` | 清空所有通话记录 |
| GET | `/api/notifications/config` | 获取多渠道通知配置 |
| POST | `/api/notifications/config` | 设置多渠道通知配置 |
| POST | `/api/notifications/test/:channel` | 测试指定通知渠道 |
| GET | `/api/vowifi/status` | 获取 VoWiFi 连接状态 |
| GET | `/api/vowifi/control` | 获取 VoWiFi 控制配置 |
| POST | `/api/vowifi/feature` | 开启/关闭 VoWiFi 功能 |
| POST | `/api/vowifi/connection` | 启用/停止 VoWiFi 隧道连接 |
| POST | `/api/vowifi/connect` | 立即手动触发 VoWiFi 连接 |
| GET | `/api/vowifi/profile` | 获取当前匹配的 VoWiFi 配置 |
| GET | `/api/vowifi/profiles` | 列出所有 VoWiFi 预设配置 |
| GET | `/api/vowifi/diagnostics` | 获取 VoWiFi 诊断详细数据 |
| GET | `/api/vowifi/events` | 获取 VoWiFi 运行日志流 |
| GET | `/api/vowifi/soak` | 获取 VoWiFi 稳定性压测记录 |
| GET | `/api/vowifi/sms/delivery` | 获取 VoWiFi 短信投递日志 |
| GET | `/api/vowifi/esim-restore/status` | 获取基带/eSIM 状态恢复进度 |

## USB 模式说明

| 模式值 | 名称 | 说明 |
|--------|------|------|
| 1 | CDC-NCM | Network Control Model |
| 2 | CDC-ECM | Ethernet Control Model |
| 3 | RNDIS | Remote NDIS |

**注意**: USB 模式切换无需重启，立即生效。

## 射频模式说明

| 模式值 | 名称 | 说明 |
|--------|------|------|
| auto | 4G/5G Auto | 4G/5G 自动切换 |
| lte | LTE Only | 仅使用 4G LTE |
| nr | NR Only | 仅使用 5G NR |

**注意**: 切换射频模式后，网络会重新注册，可能需要等待几秒。

## 频段锁定说明

频段锁定用于限制设备仅使用指定的频段连接网络，可用于优化信号或避免干扰。

### 支持的频段

**LTE 频段**:
- FDD: B1-B16 (如 B1=1800, B3=1800, B8=900)
- TDD: B33-B48 (如 B38=2600, B40=2300, B41=2500)

**NR 频段**:
- FDD: N1-N16 (如 N1=2100, N28=700)
- TDD: N41-N56+ (如 N41=2500, N77=3700, N78=3500, N79=4900)

### 频段锁定示例

**锁定 LTE B1+B3**:
```json
{
  "lte_fdd_bands": [1, 3],
  "lte_tdd_bands": [],
  "nr_fdd_bands": [],
  "nr_tdd_bands": []
}
```

**锁定 NR N78 (5G 中国移动/联通)**:
```json
{
  "lte_fdd_bands": [],
  "lte_tdd_bands": [],
  "nr_fdd_bands": [],
  "nr_tdd_bands": [78]
}
```

**混合锁定 LTE B1+B3+B38+B40 和 NR N78+N79**:
```json
{
  "lte_fdd_bands": [1, 3],
  "lte_tdd_bands": [38, 40],
  "nr_fdd_bands": [],
  "nr_tdd_bands": [78, 79]
}
```

**解除所有频段锁定**:
```json
{
  "lte_fdd_bands": [],
  "lte_tdd_bands": [],
  "nr_fdd_bands": [],
  "nr_tdd_bands": []
}
```

**注意**: 频段锁定可能导致无法连接网络，请确保锁定的频段在当地有信号覆盖。

## 环境变量（可选）

你可以在 Bruno 中配置环境变量来管理不同的服务器地址：

1. 创建环境（如 "Development", "Production"）
2. 设置变量：
   - `base_url`: `http://192.168.68.1:3000`
3. 在请求中使用：`{{base_url}}/api/health`

## 响应格式

所有接口返回统一的 JSON 格式：

### 成功响应
```json
{
  "status": "ok",
  "message": "Success",
  "data": { ... }
}
```

### 错误响应
```json
{
  "status": "error",
  "message": "错误信息"
}
```

## 注意事项

1. 确保后端服务已启动：`./start.sh`
2. 确认网络连接正常
3. 某些接口需要硬件支持（如 USB 模式切换）
4. AT 指令需要 Modem 在线

## 更多信息

查看项目主 README 了解更多详情。
